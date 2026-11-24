# ManGloss

ManGloss 是用 Rust 编写的本机 man 手册阅读器。命令名是 `mangl`，默认双语，使用你自己的阿里云机器翻译通用版凭证。

## 安装

从 [GitHub Releases](https://github.com/jiamadi/mangloss/releases) 下载与你的系统和 CPU 对应的 `mangl-v版本-平台.tar.gz`。可用 `uname -m` 查看架构。

| 系统 | CPU | 文件名末尾 |
| --- | --- | --- |
| macOS | Apple Silicon | `macos-arm64.tar.gz` |
| macOS | Intel | `macos-x86_64.tar.gz` |
| Linux / WSL | x86_64 | `linux-x86_64.tar.gz` |
| Linux / WSL | aarch64 | `linux-arm64.tar.gz` |

解压后安装到自己的目录，把下面的文件名替换成你下载的版本和平台。

```sh
tar -xzf mangl-v0.1.0-macos-arm64.tar.gz
mkdir -p ~/.local/bin
install -m 755 mangl ~/.local/bin/mangl
export PATH="$HOME/.local/bin:$PATH"
mangl --version
```

可以把 `export PATH` 这行写到 shell 的启动配置中。发行包附带 `SHA256SUMS`。Linux 发行包在 Ubuntu 22.04 构建，使用 glibc。在精简的 Linux 或 WSL 上需要先安装 `man` 和想阅读的手册，例如在 Debian 或 Ubuntu 上安装 `man-db` 和 `manpages`。

## 使用

```sh
mangl ls           # 默认双语，等同于 mangl -b ls
mangl -o ls        # 仅原文，离线
mangl -t ls        # 仅译文
mangl -b 3 printf  # 指定第 3 节，双语
mangl config       # 配置阿里云翻译和目标语言
```

三种模式互斥，只影响当次运行。阅读时用方向键或 `j` / `k` 滚动，`PgUp` / `PgDn` 翻页，`q` 退出。双语和仅译文模式下可按 `c` 进入配置，按 `r` 重试失败的翻译。未配置时可按 `s` 跳过首次引导。重定向或管道中输出纯文本，配置向导只在交互终端运行。

翻译时优先处理当前屏附近的段落，逐批显示并缓存成功的译文。仅译文模式下，未完成和失败的段落显示状态，命令及识别出的代码示例原样保留。首版基于系统 `man` 的文本布局识别内容块，行内代码和特殊版式的识别取决于手册的排版。

## 配置翻译

运行 `mangl config`，按提示打开阿里云控制台，填写目标语言和 RAM 用户的 AccessKey ID / Secret。默认目标语言为简体中文（`zh`）。凭证需要 `alimt:GetBatchTranslate` 权限。向导会用固定短句调用一次服务验证配置。翻译模式会把需要翻译的手册段落发给阿里云，调用会使用你的账户额度。

凭证保存在 macOS Keychain 或 Linux Secret Service。Linux 上使用翻译需要可用且已解锁的 Secret Service（例如 GNOME Keyring）。没有凭证服务的环境仍可用 `mangl -o` 阅读原文。

- [开通阿里云机器翻译](https://help.aliyun.com/zh/machine-translation/getting-started/activate-machine-translation-for-developers)
- [目标语言代码](https://help.aliyun.com/zh/machine-translation/developer-reference/machine-translation-language-code-list)

## 开发与发布

```sh
cargo test --locked
cargo run --locked -- -o ls
```

从 [src/main.rs](src/main.rs) 进入，然后看 [src/cli.rs](src/cli.rs) 如何解析参数、[src/manual.rs](src/manual.rs) 如何把手册拆成内容块，以及 [src/translation/mod.rs](src/translation/mod.rs) 如何用线程与通道把翻译结果送回终端界面。

版本号与 `Cargo.toml` 一致的 `v*` tag 会触发 [Release 工作流](.github/workflows/release.yml)。四个构建通过测试和离线阅读检查后，工作流把安装包和校验文件上传到 GitHub Release 草稿，供维护者核对并发布。
