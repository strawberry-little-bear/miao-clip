# MiaoClip 妙夹

一个面向个人使用的本地优先剪贴板工具箱，把复制到剪贴板的内容变成可以理解、处理和复用的本地信息。

MiaoClip 是一个独立重新实现的项目。它保留剪贴板工具最有价值的基础能力，同时把重点放在更贴近日常工作的内容理解和轻量操作上，而不是复刻 CopyQ 的插件体系，也不是朋友的 `Ctrl-CCCC` 项目的代码分支。

## 为什么做 MiaoClip

复制过的内容经常会变成一次性信息：一段 JSON 需要整理，一个 URL 需要识别，一段文本需要清洁，账号和密码又需要被谨慎地区分开。MiaoClip 希望让这些内容在本机形成一个清晰、可控的工作区：

- 普通剪贴板内容可以被搜索、预览和再次使用
- 内容可以按类型触发合适的下一步动作
- 普通历史和敏感内容分开处理
- 数据优先留在用户自己的设备上
- 功能保持轻量，不要求用户维护复杂的服务端或前端构建链

## 当前版本

当前是 `0.1.0`，用于验证桌面应用的最小闭环：

- 读取当前文本剪贴板
- 将文本写回系统剪贴板
- 识别空文本、普通文本和 JSON
- 格式化 JSON
- 清洁多余空白和换行
- 在本地工作区中查看最近读取的内容
- 搜索当前会话中的内容
- 查看选中内容的类型、长度和预览
- 使用 macOS 和 Linux 友好的 Tauri 桌面壳

当前会话记录还没有写入持久化存储；关闭应用后，这一版的会话记录会消失。持久化历史会在下一阶段加入，并且不会把疑似密码、Token 或 API Key 直接混入普通历史文件。

## 技术结构

- **桌面壳**：Tauri 2
- **核心逻辑**：Rust
- **系统剪贴板**：`arboard`
- **界面**：原生 HTML、CSS 和 JavaScript
- **前端资源**：直接使用 `ui/` 静态目录

本项目当前不需要 Node.js、npm 或前端打包工具。Tauri 会直接把 `ui/` 作为静态资源打包。后续如果加入更复杂的组件、类型检查或测试工具，再单独评估是否引入 Node.js 构建链；现在不会为了工具链本身增加维护成本。

## 开发环境

需要：

- Rust stable toolchain
- Tauri CLI 2
- 当前操作系统对应的 Tauri WebView 和桌面开发依赖

macOS 通常需要 Xcode Command Line Tools。Linux 需要安装发行版对应的 GTK、WebKitGTK 和开发库；请按 Tauri 官方的 Linux prerequisites 配置。

检查 Rust 与 Cargo：

```bash
rustc --version
cargo --version
```

安装 Tauri CLI（如果本机还没有 `cargo tauri`）：

```bash
cargo install tauri-cli --version '^2'
```

## 启动项目

在仓库根目录执行：

```bash
cargo tauri dev
```

Rust 项目位于 `src-tauri/`，静态界面位于 `ui/`。如果只需要检查 Rust 代码，可以执行：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

## 本地优先与敏感内容策略

MiaoClip 会优先使用本地数据，不要求账号、云端同步或远程服务。这个原则不等于“所有内容都可以直接写入一个 JSON 文件”：

1. 普通文本历史可以存放在 Tauri 的应用数据目录中。
2. 疑似密码、Token、API Key、Bearer Token 等内容需要单独识别。
3. 敏感内容不直接进入普通历史文件，后续将接入系统安全存储或明确的用户确认流程。
4. 账号与密码之间的关联推荐必须由用户主动确认，不能因为一次复制就自动保存秘密。

在这些边界实现之前，MiaoClip 不会把“敏感内容存储”宣传成已经完成的功能。

## 路线图

- [ ] 本地持久化普通剪贴板历史
- [ ] 普通历史去重、排序、固定和清理
- [ ] URL、代码、JSON、账号和敏感内容识别
- [ ] 敏感内容独立存储，并接入系统安全存储
- [ ] 根据账号、域名等上下文提供可控的关联推荐
- [ ] 图片和文件剪贴板的本地索引
- [ ] 全局快捷键与快速粘贴
- [ ] macOS 和 Linux 的打包、安装与实际设备测试
- [ ] 面向贡献者的测试与开发文档

路线图是方向，不代表所有项目都会按当前顺序实现。每项功能都要先确认本地隐私边界和日常使用价值。

## 参与开发

这是 `strawberry-little-bear` 账号下的独立项目。欢迎通过 Issue 讨论使用场景、平台问题和功能建议，也欢迎提交小而明确的 Pull Request。

提交代码前建议运行：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
```

## 许可证

本项目使用 [MIT License](LICENSE)。
