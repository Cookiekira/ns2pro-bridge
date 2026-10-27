//! Native usbip-win2 attach. All device discovery/IOCTL waits run on a blocking
//! worker, leaving the shared USB/IP TCP listener responsive during import.
use super::AttachError;
use ns2pro_usb::usbip_win as wire;
use std::{
    mem::size_of,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
            SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
            SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        },
        Foundation::{
            ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, ERROR_NO_MORE_ITEMS, GENERIC_READ,
            GENERIC_WRITE, HANDLE, RtlNtStatusToDosError, STATUS_BUFFER_TOO_SMALL,
            STATUS_INVALID_BUFFER_SIZE, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED},
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
    core::{GUID, PCWSTR},
};

const DEVICE_GUID: GUID = GUID::from_u128(0xb403_0c06_dc5f_4fcc_87eb_e551_5a09_35c0);
fn platform(error: impl std::fmt::Display) -> AttachError {
    AttachError::Platform(format!(
        "{error}; install/start the usbip-win2 driver (supported released ABI: v.0.9.6.1–v.0.9.8.1), check device access permissions, or use --no-auto-attach"
    ))
}
fn ioctl_error(error: windows::core::Error) -> AttachError {
    let code = error.code().0.cast_unsigned();
    // HRESULT_FROM_WIN32: facility 7 carries a Win32 error code in the low 16 bits.
    if code & 0xffff_0000 == 0x8007_0000 {
        AttachError::Io(std::io::Error::from_raw_os_error(
            (code & 0xffff).cast_signed(),
        ))
    } else {
        platform(error)
    }
}
fn size_rejected(error: &AttachError) -> bool {
    let AttachError::Io(error) = error else {
        return false;
    };
    // These statuses are returned before the driver starts a network import.
    [STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_BUFFER_SIZE]
        .into_iter()
        .any(|status| error.raw_os_error() == Some(dos_error(status)))
}
fn dos_error(status: windows::Win32::Foundation::NTSTATUS) -> i32 {
    // SAFETY: RtlNtStatusToDosError is a pure table lookup on an integer status.
    unsafe { RtlNtStatusToDosError(status) }.cast_signed()
}
/// `cbSize` value for a fixed-size Win32 structure.
#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded by the compile-time assertion"
)]
const fn cb_size<T>() -> u32 {
    const { assert!(size_of::<T>() <= u16::MAX as usize) };
    size_of::<T>() as u32
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: only called on newly returned successful CreateFile/CreateEvent handles.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
struct DeviceList(HDEVINFO);
impl Drop for DeviceList {
    fn drop(&mut self) {
        // SAFETY: self.0 came from a successful SetupDiGetClassDevsW and is destroyed
        // exactly once, here.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}
fn open_driver() -> Result<OwnedHandle, AttachError> {
    // SAFETY: DEVICE_GUID is borrowed for the call; a null enumerator and no parent
    // window are documented defaults.
    let list = unsafe {
        SetupDiGetClassDevsW(
            Some(&DEVICE_GUID),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    let devices = DeviceList(list.map_err(platform)?);
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: cb_size::<SP_DEVICE_INTERFACE_DATA>(),
        ..Default::default()
    };
    // SAFETY: devices.0 is a live device information set and interface is writable
    // storage whose cbSize is set as the API requires.
    let found = unsafe {
        SetupDiEnumDeviceInterfaces(devices.0, None, &DEVICE_GUID, 0, &raw mut interface)
    };
    if let Err(error) = found {
        if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
            return Err(platform("usbip-win2 device interface not found"));
        }
        return Err(platform(error));
    }
    let mut required = 0;
    // SAFETY: a size query: no detail buffer is passed and required is writable.
    let probe = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            devices.0,
            &raw const interface,
            None,
            0,
            Some(&raw mut required),
            None,
        )
    };
    if let Err(error) = probe
        && error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(platform(error));
    }
    if required < cb_size::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() || required > 1024 * 1024 {
        return Err(platform("invalid SetupDi device path size"));
    }
    let required_len =
        usize::try_from(required).map_err(|_| platform("invalid SetupDi device path size"))?;
    // u32 storage gives the detail header its required alignment, including cbSize.
    let mut storage = vec![0u32; required_len.div_ceil(4)];
    let detail = storage
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    // SAFETY: storage is at least `required` >= size_of::<detail header>() bytes and
    // 4-byte aligned, so the header write is in bounds and aligned.
    unsafe { (*detail).cbSize = cb_size::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() };
    // SAFETY: detail points to `required` writable bytes, the size SetupDi asked for,
    // and interface is the element enumerated above.
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            devices.0,
            &raw const interface,
            Some(detail),
            required,
            None,
            None,
        )
    }
    .map_err(platform)?;
    // Validate termination within the allocated device path before CreateFileW.
    let path_offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let path_start = detail.cast::<u16>().wrapping_add(path_offset / 2);
    // SAFETY: DevicePath begins path_offset bytes into the `required`-byte allocation,
    // so this many u16s are in bounds, aligned and initialized (zero-filled, then
    // written by SetupDi). storage outlives the slice and is not written while it lives.
    let path = unsafe { std::slice::from_raw_parts(path_start, (required_len - path_offset) / 2) };
    if !path.contains(&0) {
        return Err(platform("unterminated SetupDi device path"));
    }
    // SAFETY: path is NUL-terminated within its slice and outlives the call; the
    // returned handle is wrapped exactly once by owned().
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            None,
        )
    }
    .map(owned)
    .map_err(platform)
}

/// Keep buffers, event and OVERLAPPED alive until Windows acknowledges completion,
/// even on cancellation. `CancelIoEx` requests cancellation; it does not complete it.
fn ioctl(
    handle: &OwnedHandle,
    code: u32,
    buffer: &mut [u8],
    output_len: u32,
    cancel: &CancellationToken,
) -> Result<u32, AttachError> {
    if cancel.is_cancelled() {
        return Err(AttachError::Cancelled);
    }
    let input_len = u32::try_from(buffer.len()).map_err(|_| platform("IOCTL buffer too large"))?;
    if output_len > input_len {
        return Err(platform("IOCTL output exceeds its buffer"));
    }
    // SAFETY: no pointers are passed (default security, unnamed); the new handle is
    // wrapped exactly once by owned().
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) };
    let event = owned(event.map_err(platform)?);
    let mut overlapped = OVERLAPPED {
        hEvent: raw(&event),
        ..Default::default()
    };
    let mut transferred = 0;
    // SAFETY: buffer is valid for reads of input_len bytes and, when output is
    // requested, writes of output_len <= input_len bytes. buffer, transferred, event
    // and overlapped all outlive the request: every path below that can leave it
    // pending waits for completion before returning.
    let result = unsafe {
        DeviceIoControl(
            raw(handle),
            code,
            Some(buffer.as_ptr().cast()),
            input_len,
            if output_len == 0 {
                None
            } else {
                Some(buffer.as_mut_ptr().cast())
            },
            output_len,
            Some(&raw mut transferred),
            Some(&raw mut overlapped),
        )
    };
    match result {
        Ok(()) => return Ok(transferred),
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(error) => return Err(ioctl_error(error)),
    }
    loop {
        // Polling is confined to driver setup/teardown, never the input path.
        // SAFETY: event is an open handle owned by this function.
        let wait = unsafe { WaitForSingleObject(raw(&event), 50) };
        if wait == WAIT_OBJECT_0 {
            break;
        }
        if cancel.is_cancelled() || wait != WAIT_TIMEOUT {
            // SAFETY: handle and overlapped identify this function's pending request,
            // and both are alive.
            let _ = unsafe { CancelIoEx(raw(handle), Some(&raw const overlapped)) };
            // Drain before freeing OVERLAPPED or the METHOD_BUFFERED request.
            // SAFETY: overlapped and transferred are alive; bWait = true blocks until
            // the request completes, so nothing it references is freed early.
            let result = unsafe {
                GetOverlappedResult(
                    raw(handle),
                    &raw const overlapped,
                    &raw mut transferred,
                    true,
                )
            };
            // Completion may have won the cancellation race. Return it so the
            // worker can detach an import that actually succeeded.
            return match result {
                Ok(()) => Ok(transferred),
                Err(_) if cancel.is_cancelled() => Err(AttachError::Cancelled),
                Err(error) => Err(ioctl_error(error)),
            };
        }
    }
    // SAFETY: the event is signalled, so the request has completed; overlapped and
    // transferred are alive for the call.
    unsafe {
        GetOverlappedResult(
            raw(handle),
            &raw const overlapped,
            &raw mut transferred,
            false,
        )
    }
    .map_err(ioctl_error)?;
    Ok(transferred)
}
fn detach_port(handle: &OwnedHandle, port: i32) -> Result<(), AttachError> {
    let mut request = wire::plugout(port).map_err(platform)?;
    ioctl(
        handle,
        wire::PLUGOUT_HARDWARE,
        &mut request,
        0,
        &CancellationToken::new(),
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct Attached {
    stop: CancellationToken,
    worker: Option<JoinHandle<Result<(), AttachError>>>,
}
impl Attached {
    /// Imports the device at `address` into usbip-win2 on a blocking worker,
    /// trying each supported driver ABI.
    ///
    /// # Errors
    ///
    /// [`AttachError::Cancelled`] when `cancel` fires first, or the driver failure
    /// that prevented the import.
    pub async fn attach(
        mut address: SocketAddr,
        cancel: CancellationToken,
    ) -> Result<Self, AttachError> {
        if cancel.is_cancelled() {
            return Err(AttachError::Cancelled);
        }
        // A wildcard listener accepts loopback but cannot be passed to getaddrinfo
        // as the destination of the driver's TCP connection.
        if address.ip().is_unspecified() {
            address.set_ip(match address {
                SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        wire::plugin(address, wire::Abi::V0981).map_err(platform)?;
        let stop = cancel.child_token();
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let imported = (|| {
                if worker_stop.is_cancelled() {
                    return Err(AttachError::Cancelled);
                }
                let handle = open_driver().map_err(|error| {
                    if worker_stop.is_cancelled() {
                        AttachError::Cancelled
                    } else {
                        error
                    }
                })?;
                let port_len = u32::try_from(wire::PORT_END).map_err(platform)?;
                for abi in wire::Abi::SUPPORTED {
                    let mut request = wire::plugin(address, abi).map_err(platform)?;
                    match ioctl(
                        &handle,
                        wire::PLUGIN_HARDWARE,
                        &mut request,
                        port_len,
                        &worker_stop,
                    ) {
                        Ok(bytes) => {
                            let port = wire::attached_port(&request, bytes).map_err(platform)?;
                            return Ok((handle, port));
                        }
                        Err(error) if size_rejected(&error) => {}
                        Err(AttachError::Cancelled) => return Err(AttachError::Cancelled),
                        Err(error) => return Err(platform(error)),
                    }
                }
                Err(platform("usbip-win2 rejected every supported request size"))
            })();
            let (handle, port) = match imported {
                Ok(imported) => imported,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return Ok(());
                }
            };
            // Receiver loss means the attach future was cancelled/dropped.
            let delivered = ready_tx.send(Ok(())).is_ok();
            if delivered {
                tracing::info!(port, %address, "Attached virtual USB device through usbip-win2");
                futures::executor::block_on(worker_stop.cancelled());
            }
            detach_port(&handle, port)
        });
        // Attached's Drop cancels the worker on every failure or dropped future.
        let mut attached = Self {
            stop,
            worker: Some(worker),
        };
        let result = ready_rx
            .await
            .map_err(|error| platform(format!("usbip-win2 worker ended: {error}")))?;
        if let Err(error) = result {
            let cancelled = attached.stop.is_cancelled();
            let _ = attached.detach().await;
            return Err(if cancelled {
                AttachError::Cancelled
            } else {
                error
            });
        }
        if attached.stop.is_cancelled() {
            attached.detach().await?;
            return Err(AttachError::Cancelled);
        }
        Ok(attached)
    }
    /// Stops the worker and waits for it to unplug the port. Idempotent.
    ///
    /// # Errors
    ///
    /// When the worker panicked or the unplug IOCTL failed.
    pub async fn detach(&mut self) -> Result<(), AttachError> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.await.map_err(platform)??;
        }
        Ok(())
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub(super) struct Adapter {
    pub listener: tokio::net::TcpListener,
}
impl super::attachment::LeaseAdapter for Adapter {
    type Lease = Attached;
    type Transport = ();

    async fn attach(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(Attached, Option<()>), AttachError> {
        let attached = Attached::attach(self.listener.local_addr()?, cancel.clone()).await?;
        Ok((attached, None))
    }
    async fn serve(
        &self,
        _: Option<()>,
        channels: &crate::channels::Channels,
        cancel: &CancellationToken,
    ) -> std::io::Result<()> {
        super::server::serve(&self.listener, channels.clone(), cancel.clone()).await
    }
    async fn detach(&self, lease: &mut Attached) -> Result<(), AttachError> {
        lease.detach().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SEM_TIMEOUT},
        System::Threading::{EVENT_MODIFY_STATE, OpenEventW},
    };

    #[test]
    fn only_pre_import_size_rejections_allow_abi_retry() {
        for status in [STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_BUFFER_SIZE] {
            let error = std::io::Error::from_raw_os_error(dos_error(status));
            assert!(size_rejected(&AttachError::Io(error)));
        }
        for code in [ERROR_ACCESS_DENIED, ERROR_SEM_TIMEOUT] {
            let error = std::io::Error::from_raw_os_error(code.0.cast_signed());
            assert!(!size_rejected(&AttachError::Io(error)));
        }
        assert!(!size_rejected(&AttachError::Cancelled));
    }

    #[test]
    fn owned_native_event_closes_exactly_at_scope_end() {
        // Numeric handle values can be reused immediately by another test thread.
        // A unique named kernel object lets us verify actual lifetime instead.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = windows::core::HSTRING::from(format!(
            "Local\\Ns2ProHandleTest-{}-{nonce}",
            std::process::id()
        ));
        // SAFETY: name is a NUL-terminated HSTRING borrowed for each call; handles are
        // wrapped exactly once by owned().
        let event = owned(unsafe { CreateEventW(None, true, false, &name) }.unwrap());
        // SAFETY: as above.
        let second = owned(unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) }.unwrap());
        drop(second);
        drop(event);
        // SAFETY: as above; the call is expected to fail.
        let error = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) }.unwrap_err();
        assert_eq!(error.code(), ERROR_FILE_NOT_FOUND.to_hresult());
    }

    #[tokio::test]
    async fn pre_cancelled_attach_never_probes_the_driver() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        // Port zero would otherwise fail argument validation; cancellation wins
        // before any encoding, SetupDi discovery or device open is attempted.
        let result = Attached::attach("127.0.0.1:0".parse().unwrap(), cancel).await;
        assert!(matches!(result, Err(AttachError::Cancelled)));
    }

    #[tokio::test]
    async fn detach_is_idempotent_and_waits_for_worker_resource_release() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let released = Arc::new(AtomicBool::new(false));
        let worker_released = released.clone();
        let worker = tokio::task::spawn_blocking(move || {
            futures::executor::block_on(worker_stop.cancelled());
            worker_released.store(true, Ordering::SeqCst);
            Ok(())
        });
        let mut attached = Attached {
            stop,
            worker: Some(worker),
        };
        attached.detach().await.unwrap();
        assert!(released.load(Ordering::SeqCst));
        attached.detach().await.unwrap();
    }
}
