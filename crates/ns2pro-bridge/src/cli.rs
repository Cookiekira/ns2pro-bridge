//! Command-line options.
use clap::Parser;
use ns2pro_proto::Address;
use std::path::PathBuf;
/// Command-line options.
#[derive(Clone, Debug, Parser)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line flag"
)]
#[command(version, about = "Native Switch 2 Pro BLE to USB bridge")]
pub struct Options {
    /// USB/IP listen address (HOST:PORT) for manual Linux and Windows serving
    #[arg(long, default_value = "localhost:3241", value_parser=parse_usb_addr)]
    pub usb_addr: String,
    /// Controller address; overrides cache and discovery selection
    #[arg(long)]
    pub device_address: Option<Address>,
    /// Pair again, once successfully per process across reconnects
    #[arg(long)]
    pub pair_host: bool,
    /// Pairing host identity override; bypasses adapter identity lookup
    #[arg(long)]
    pub host_address: Option<Address>,
    /// Remove the cached selection before reconnect supervision
    #[arg(long)]
    pub forget_device: bool,
    /// Controller cache file [default: /var/lib/ns2pro-bridge/controller-cache.json on
    /// Linux, ns2pro-bridge\controller-cache.json under the local app data folder on Windows]
    #[arg(long)]
    pub cache_file: Option<PathBuf>,
    /// Serve TCP USB/IP for manual/remote clients without attaching
    #[arg(long)]
    pub no_auto_attach: bool,
    /// Feature flags (buttons, sticks, IMU); hexadecimal or decimal
    #[arg(long,default_value=default_feature_flags(),value_parser=parse_flags)]
    pub feature_flags: u8,
    /// Log level: trace, debug, info, warn, error or off; `RUST_LOG` takes precedence
    #[arg(long, default_value = "info",value_parser=parse_level)]
    pub log_level: String,
    /// Hidden: scan briefly to check Bluetooth access, then exit.
    #[arg(long, hide = true)]
    pub bt_smoke_test: bool,
}
impl Options {
    /// The explicit `--cache-file`, or the platform default.
    ///
    /// # Errors
    ///
    /// On Windows, when the local application data folder cannot be resolved.
    pub fn cache_file(&self) -> std::io::Result<PathBuf> {
        if let Some(path) = &self.cache_file {
            return Ok(path.clone());
        }
        #[cfg(target_os = "linux")]
        {
            Ok(PathBuf::from(
                "/var/lib/ns2pro-bridge/controller-cache.json",
            ))
        }
        #[cfg(windows)]
        {
            dirs::data_local_dir()
                .map(|dir| dir.join("ns2pro-bridge/controller-cache.json"))
                .ok_or_else(|| {
                    std::io::Error::other(
                        "cannot resolve the local app data folder; pass --cache-file",
                    )
                })
        }
    }
}
fn parse_flags(s: &str) -> Result<u8, String> {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .map_or_else(|| s.parse(), |hex| u8::from_str_radix(hex, 16))
        .map_err(|e| e.to_string())
}

fn parse_usb_addr(s: &str) -> Result<String, String> {
    use std::net::ToSocketAddrs;
    s.to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .map(|_| s.to_owned())
        .ok_or_else(|| "expected host:port such as localhost:3241".into())
}
fn parse_level(s: &str) -> Result<String, String> {
    let value = s.to_ascii_lowercase();
    if ["trace", "debug", "info", "warn", "error", "off"].contains(&value.as_str()) {
        Ok(value)
    } else {
        Err("expected trace, debug, info, warn or error".into())
    }
}

fn default_feature_flags() -> &'static str {
    static DEFAULT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| format!("0x{:02x}", ns2pro_proto::command::DEFAULT_FEATURE_FLAGS))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_default_and_override() {
        let defaults = Options::try_parse_from(["bridge"]).unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            defaults.cache_file().unwrap(),
            PathBuf::from("/var/lib/ns2pro-bridge/controller-cache.json")
        );
        #[cfg(windows)]
        assert_eq!(
            defaults.cache_file().unwrap(),
            dirs::data_local_dir()
                .unwrap()
                .join("ns2pro-bridge/controller-cache.json")
        );
        let options =
            Options::try_parse_from(["bridge", "--cache-file", "/tmp/controller.json"]).unwrap();
        assert_eq!(
            options.cache_file().unwrap(),
            PathBuf::from("/tmp/controller.json")
        );
    }
    #[test]
    fn defaults_and_flags() {
        let o = Options::try_parse_from(["bridge"]).unwrap();
        assert_eq!(o.usb_addr, "localhost:3241");
        assert_eq!(o.feature_flags, 7);
        assert_eq!(o.log_level, "info");
        assert_eq!(
            Options::try_parse_from(["bridge", "--feature-flags", "255"])
                .unwrap()
                .feature_flags,
            255
        );
        assert!(Options::try_parse_from(["bridge", "--verify-native-library"]).is_err());
    }
}
