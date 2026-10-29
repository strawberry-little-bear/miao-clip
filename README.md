# MiaoClip 妙夹

![MiaoClip](https://img.shields.io/badge/MiaoClip-%E5%A6%99%E5%A4%B9-e85d75?style=flat-square)
![Tauri](https://img.shields.io/badge/Tauri-2-24c8db?style=flat-square&logo=tauri&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-2024-000000?style=flat-square&logo=rust&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-2f855a?style=flat-square)

**一个面向个人工作流的本地优先剪贴板工作区。**

复制过的文字、链接、账号、密码、图片和文件，不应该在下一次复制后就消失。MiaoClip 会自动捕获剪贴板内容，在本机整理、搜索、预览和再次使用，让剪贴板从“一次性中转站”变成一个轻量、可控的收件箱。

> MiaoClip 是独立重新实现的项目，不是 `Ctrl-CCCC` 的代码分支，也不是 CopyQ 的复刻。它保留自动监听、历史记录和再次复制这些基础能力，但更关注个人场景下的内容识别、隐私边界和简单易用。

## 你可以用它做什么

- 复制后自动捕获，不需要反复点击“读取当前剪贴板”
- 保存并恢复最近的本地剪贴板历史，重启应用后记录仍然存在
- 自动区分普通文本、URL、账号、密码、图片和文件
- 密码、Token、API Key 等疑似敏感内容默认遮罩，可临时显示并复制原文
- 图片保留本地预览，文件记录显示文件名而不是冗长路径
- 搜索、按类型筛选、按时间筛选、收藏和删除历史记录
- 显示复制来源应用、捕获时间、内容类型和敏感状态
- 通过桌面浮层提示复制成功，支持设置通知开关、位置和持续时间
- 支持暂停监听和隐私模式，避免私人内容写入历史或弹出通知
- 支持开机自动启动设置
- 支持 macOS、Linux 和 Windows 的跨平台构建

## 界面概览

MiaoClip 的工作区围绕三个动作展开：**自动收件、快速理解、再次使用**。

| 区域 | 作用 |
| --- | --- |
| 收件箱 | 查看最近捕获的内容，并按类型或时间筛选 |
| 详情面板 | 预览内容、查看来源应用、显示敏感状态和可用操作 |
| 收藏 | 保存需要反复使用的账号、链接、模板和文本 |
| 监听开关 | 在“正在监听”和“已暂停监听”之间切换 |
| 隐私模式 | 暂停捕获、停止通知、不写入新记录，退出后恢复监听 |
| 设置 | 管理桌面提示、敏感内容提醒、提示位置、持续时间和开机启动 |

## 平台支持

| 平台 | 状态 | 说明 |
| --- | --- | --- |
| macOS | 可开发、可构建 | 当前主要开发环境，使用原生前台应用识别和 Tauri 桌面壳 |
| Linux | 可构建 | 来源应用识别优先使用 `xdotool`，不同桌面环境可能需要额外配置 |
| Windows | CI 构建支持 | 使用 Windows 原生窗口和进程 API 识别来源应用，并由 GitHub Actions 验证构建 |

Windows 版本不依赖 macOS 私有 API。仓库已配置 `windows-latest` 构建任务，会执行 Rust 检查、测试和 Tauri 桌面包构建。当前开发机是 macOS，因此 Windows 的 GUI 运行验证交给 Windows runner；发布前仍建议在真实 Windows 设备上测试剪贴板、通知、开机启动和 WebView2 行为。

## 快速开始

### 环境要求

- Rust stable toolchain
- Tauri CLI 2
- 对应操作系统的 Tauri 桌面依赖

macOS 通常需要 Xcode Command Line Tools。Linux 需要 GTK、WebKitGTK、AppIndicator 和 librsvg 等开发库。Windows 需要支持 WebView2 的系统环境，现代 Windows 10/11 通常已经具备；缺少时可安装 Microsoft Edge WebView2 Runtime。

检查 Rust：

```bash
rustc --version
cargo --version
```

安装 Tauri CLI：

```bash
cargo install tauri-cli --version '^2'
```

### 启动开发版本

在仓库根目录执行：

```bash
cargo tauri dev
```

MiaoClip 不需要 Node.js、npm 或前端打包工具。Tauri 会直接将 `ui/` 目录作为静态资源加载；Rust 核心逻辑位于 `src-tauri/`。

### 构建桌面安装包

macOS、Linux 或 Windows 环境均可执行：

```bash
cargo tauri build
```

调试构建：

```bash
cargo tauri build --debug
```

在 macOS 上通常会得到 `.app` 和 `.dmg` 相关产物；Windows 产物由 Tauri bundler 根据配置生成，通常包括 `.msi` 或 NSIS 安装包。Windows 构建也可以直接使用 GitHub Actions，无需在 macOS 上安装 Windows 交叉编译环境。

## 本地数据与隐私

MiaoClip 不需要账号、云端服务或远程数据库，剪贴板数据默认只保存在当前设备的 Tauri 应用数据目录中：

```text
history.json       剪贴板历史
images/            图片本地副本
notifications.json 通知和提示设置
```

当前版本最多保留 100 条去重后的历史记录。每条记录可以包含：

- 内容和内容类型
- 捕获时间
- 来源应用名称
- 是否敏感
- 是否收藏
- 图片尺寸或文件列表等必要元数据

### 敏感内容策略

MiaoClip 会根据密码字段、Token、API Key、Bearer Token 等特征判断文本是否疑似敏感。识别结果用于展示和提醒策略，不代表安全产品级别的密码检测：

- 列表和桌面提示默认显示遮罩文案
- 详情页可以点击眼睛按钮临时显示原文
- 显示原文后可以复制到系统剪贴板
- 敏感内容仍可按用户需要收藏、搜索和再次使用

当前历史文件是本地 JSON，并不等同于系统级加密存储。未来会评估接入 macOS Keychain、Windows Credential Manager 和 Linux Secret Service，为敏感记录提供更高等级的保护。

### 隐私模式

隐私模式适合复制银行卡号、身份证、私人聊天或其他不希望进入历史的内容。开启后：

- 暂停自动捕获
- 不弹桌面通知
- 不写入新记录
- 退出隐私模式后自动恢复监听

## 技术结构

```text
MiaoClip/
├── ui/                 原生 HTML、CSS、JavaScript 界面
├── src-tauri/
│   ├── src/lib.rs      剪贴板监听、识别、持久化和 Tauri 命令
│   ├── tauri.conf.json 桌面窗口、资源和打包配置
│   └── Cargo.toml      Rust 依赖与平台配置
└── .github/workflows/  macOS、Linux、Windows CI 与构建任务
```

核心技术：

- [Tauri 2](https://tauri.app/)：跨平台桌面壳
- [Rust](https://www.rust-lang.org/)：剪贴板监听、内容识别和本地存储
- `arboard`、`clipboard-rs`、`clipboard-master`：剪贴板读写和变化监听
- 原生 HTML、CSS、JavaScript：轻量界面，不引入前端构建链
- Windows 原生 API：获取前台窗口及来源进程名称

## 开发检查

提交前建议执行：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
git diff --check
```

GitHub Actions 会在 `ubuntu-latest`、`macos-latest` 和 `windows-latest` 上执行格式检查、编译检查和测试，并在 macOS 与 Windows runner 上构建 Tauri 桌面包。

## 路线图

- [x] 本地历史持久化、去重和数量限制
- [x] 自动监听文本、URL、账号、密码、图片和文件
- [x] 图片本地副本与预览、文件名展示
- [x] 敏感内容遮罩、临时显示和复制
- [x] 桌面浮层提示、提示位置和持续时间设置
- [x] 暂停监听和隐私模式
- [x] 来源应用记录
- [x] macOS、Linux、Windows CI 构建验证
- [ ] 系统安全存储，减少敏感内容明文落盘
- [ ] 全局快捷键与快速粘贴
- [ ] 根据账号、域名和内容上下文提供可控推荐
- [ ] 更完整的跨平台安装包发布流程

## 参与贡献

欢迎通过 [Issues](https://github.com/strawberry-little-bear/miao-clip/issues) 反馈平台问题、使用场景和功能建议，也欢迎提交小而明确的 Pull Request。

提交前请确保：

1. 变更没有绕过隐私模式和敏感内容保护边界。
2. macOS 本地检查通过，平台专属代码由 CI 覆盖。
3. README、测试和用户可见行为与变更保持一致。

## 许可证

本项目使用 [MIT License](LICENSE)。

由 `strawberry-little-bear` 创建和维护。
