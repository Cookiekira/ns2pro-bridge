# NS2Pro Bridge — Switch 2 Pro 手柄通过蓝牙无线连接电脑

[English](README.md) | [简体中文](README.zh-CN.md)

使用 `ns2pro-bridge`，让 **Nintendo Switch 2 Pro（NS2 Pro / NS2Pro）手柄通过蓝牙 BLE 无线连接电脑**。桥接程序将无线手柄呈现为有线 USB Switch 2 Pro 手柄（USB `057e:2069`），供 Steam 和 SDL 识别。

Linux 已经过实机测试。Windows 目前仅通过 CI 构建和测试，尚未验证真实蓝牙设备和驱动环境。

## 在 Linux 上通过蓝牙连接

1. 使用 `cargo binstall ns2pro-bridge` 安装，或运行安装脚本：
   ```sh
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Cookiekira/ns2pro-bridge/releases/latest/download/ns2pro-bridge-installer.sh | sh
   ```
   也可以从 [Releases](https://github.com/Cookiekira/ns2pro-bridge/releases/latest) 下载并解压 `ns2pro-bridge-x86_64-unknown-linux-gnu.tar.xz` 或 `ns2pro-bridge-aarch64-unknown-linux-gnu.tar.xz`，手动安装。
2. 运行 `ns2pro-bridge`（手动解压后运行 `./ns2pro-bridge`）。按提示输入密码，程序会自动加载虚拟 USB 驱动。
3. **首次连接时**，按下手柄顶部的**同步键（SYNC）**进行配对。程序会记住手柄。
4. 等待出现 `controller initialized; receiving input`，然后打开 Steam 开始游戏。

下次使用时，运行第 2 步并按 **Home** 键唤醒手柄。按 **Ctrl+C** 停止；收到 SIGTERM 或关闭终端（SIGHUP）时，程序也会正常分离虚拟手柄。

Linux 二进制文件要求 glibc ≥ 2.39。
输入密码后，程序以 root 身份运行，请将其保存在只有你能写入的位置。
终端内使用 `sudo` 提权；标准输入不是 TTY 时使用 `pkexec`。

## 在 Windows 上通过蓝牙连接

1. 安装已签名的 [usbip-win2](https://github.com/vadimgrn/usbip-win2) 驱动（支持 v0.9.6.1–v0.9.8.1）。
2. 使用 `cargo binstall ns2pro-bridge` 安装，或运行 PowerShell 安装脚本：
   ```powershell
   irm https://github.com/Cookiekira/ns2pro-bridge/releases/latest/download/ns2pro-bridge-installer.ps1 | iex
   ```
   也可以从 [Releases](https://github.com/Cookiekira/ns2pro-bridge/releases/latest) 下载并解压 `ns2pro-bridge-x86_64-pc-windows-msvc.zip`，手动安装。
3. 运行程序，首次连接时按下手柄的**同步键（SYNC）**。

Windows 目前仅通过原生 CI 构建和测试，尚未在真实 Windows 蓝牙设备或驱动环境中测试。

## 常见问题

### NS2 Pro 手柄怎么无线连接电脑？

按上方 Linux 或 Windows 步骤安装桥接程序，运行 `ns2pro-bridge`，首次连接时按下手柄同步键。后续使用时，运行程序并按 Home 键唤醒手柄即可。

### Steam 能识别通过蓝牙连接的手柄吗？

桥接程序负责蓝牙 BLE 连接，并向 Steam 和 SDL 提供虚拟 USB Switch 2 Pro 手柄。Steam 识别到的是任天堂手柄，而不是模拟的 Xbox 手柄。

## 故障排查

| 遇到的问题 | 解决方法 |
| --- | --- |
| 启动时提权失败或权限不足 | 确认已安装 `sudo`（终端内）或 `pkexec`（无 TTY），且允许提权。 |
| `vhci` 或挂载错误 | 确认内核提供 `vhci-hcd`，并已安装 `modprobe`；程序会自动加载模块。 |
| Steam 能看到手柄但没有输入（Linux，32 位 Steam） | 执行 `sudo pacman -S --needed lib32-libusb`（Arch/CachyOS；其他发行版请安装对应的 32 位 libusb），然后完全重启 Steam。 |
| `kernel removed virtual controller` | 虚拟 USB 设备已被分离，请重新启动程序。 |

添加 `--log-level debug` 查看调试信息。日志每 5 秒输出报告频率、延迟 p50/p99 和实际连接间隔。

## 在另一台电脑上使用手柄

1. 在有蓝牙的电脑上运行：`ns2pro-bridge --no-auto-attach --usb-addr 0.0.0.0:3241`
2. 在另一台电脑上运行：`sudo usbip --tcp-port 3241 attach -r HOST -b 1-1`

USB/IP 服务没有身份验证，请仅向可信任的电脑开放。

## 命令行选项

| 选项 | 默认值 | 说明 |
| --- | --- | --- |
| `--forget-device` | 关闭 | 忘记已保存的手柄并重新配对 |
| `--device-address MAC` | 使用缓存 | 连接指定地址的手柄 |
| `--no-auto-attach` | 关闭 | 通过 TCP 提供 USB/IP 服务，不在本机挂载 |
| `--usb-addr HOST:PORT` | `localhost:3241` | USB/IP 服务地址 |
| `--log-level LEVEL` | `info` | `trace`/`debug`/`info`/`warn`/`error`/`off`；`RUST_LOG` 优先 |

其他选项：

| 选项 | 默认值 | 说明 |
| --- | --- | --- |
| `--pair-host` | 关闭 | 每次运行时重新配对主机一次 |
| `--host-address MAC` | 适配器地址 | 配对时使用的主机地址 |
| `--cache-file PATH` | Linux：`/var/lib/ns2pro-bridge/controller-cache.json`；Windows：本地应用数据目录下的 `ns2pro-bridge/controller-cache.json` | 保存手柄信息，JSON 格式：`{"address":"AA:BB:CC:DD:EE:FF"}` |
| `--feature-flags FLAGS` | `0x07` | 按键、摇杆、IMU 功能标志（十六进制或十进制） |

退出码：`0` 表示正常退出，`1` 表示运行时错误，`2` 表示参数或 USB 设置错误。选项区分大小写，不支持 `/?` 别名。

## 性能与兼容性

- **延迟：** 在已测试的电脑上，桥接程序自身增加约 25–50 µs 的延迟（p99）。
- **蓝牙间隔：** Linux 会请求 **7.5 ms** 的连接间隔（默认通常为 48.75 ms），并记录实际结果。已验证的适配器均不支持 5 ms。
- **断线处理：** 按键和摇杆立即恢复中立状态。程序会自动重连、恢复玩家指示灯并丢弃旧的震动指令。
- **已测试硬件：** Linux x64，搭配 MediaTek MT7922 适配器。arm64 和 Windows 目前仅通过 CI 验证。

## 从源码构建

```sh
cargo build --locked --release -p ns2pro-bridge
cargo test --locked --workspace
```

输出文件：`target/release/ns2pro-bridge`（Windows 下带 `.exe` 后缀）。需要稳定版 Rust（edition 2024）。

发布流程：合并 release-plz PR 后，发布工作区中的 crate 并创建 `v<version>` 标签，随后由 dist 构建 GitHub Release。需要配置 `CARGO_REGISTRY_TOKEN` secret（crates.io 的 publish-new/publish-update 权限）和 `RELEASE_PLZ_TOKEN` secret（使用对此仓库具有 Contents 和 Pull requests 读写权限的 [GitHub 细粒度 PAT](https://release-plz.dev/docs/github/token#use-a-personal-access-token)，以便标签触发 dist）。

## 致谢与许可证

- **[VIIPER](https://github.com/Alia5/VIIPER)**，作者 Alia5（GPL-3.0）：虚拟 USB 设备模型、描述符和测试数据移植自 VIIPER。许可证见 [LICENSES/VIIPER-GPL-3.0.txt](LICENSES/VIIPER-GPL-3.0.txt)。
- **[switch2_controller_research](https://github.com/ndeadly/switch2_controller_research)**：手柄协议研究。
- **[usbip-win2](https://github.com/vadimgrn/usbip-win2)**：Windows USB/IP 驱动（需单独安装）。

本项目采用 GPL-3.0-or-later 许可证，详见 [LICENSE.txt](LICENSE.txt) 和 [THIRD_PARTY.md](THIRD_PARTY.md)。
