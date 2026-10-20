# 本地验证与发布

只分发 CLI 二进制和技能包。`Cargo.toml` 的 `publish = false` 禁止误发 crates.io。
开发用发布脚本需要 Python 3.11+ 标准库；最终安装脚本和 CLI 不需要 Python。

## 自动流程

- CI：Linux Rust 1.86 / stable 的 fmt、Clippy、强制媒体测试；三个分发平台的原生构建、
  完整测试、二进制打包、校验和及离线安装/运行验证。
- Release：推送 `v*` tag 触发；校验 tag 必须等于 `v` 加 Cargo 版本，Cargo.lock 与
  Changelog 必须匹配。复用全部 CI，通过后汇总三个平台和技能包，生成 SHA256SUMS。
- 只有 tag push 会创建 GitHub Release。手动运行 Release workflow 只生成可下载的
  `release-bundle` Actions artifact，适合正式发布前演练。
- 发布使用现有 tag，不替你生成 tag；已存在的 Release 不覆盖。预发布版本含 `-`，
  如 `0.1.0-rc.1`，发布为 prerelease。CI 权限只读，仅最终发布 job 有 contents:write。
- 使用仓库自带 GITHUB_TOKEN，无需 crates.io token；Actions 必须在远端仓库启用。

## 本地演练

```sh
python3 scripts/release.py check --tag v0.1.0
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
CUTCRAFT_REQUIRE_FFMPEG=1 cargo test --locked
python3 scripts/release.py package --target aarch64-apple-darwin
python3 scripts/release.py assemble --partial
python3 scripts/test-release.py --dist dist
```

`--target` 需匹配当前 Rust host；Linux 和 Intel Mac 分别使用其 target。
macOS 正式构建设置 `MACOSX_DEPLOYMENT_TARGET=15.0`。本地没有设置时，使用工具链默认值。
打包脚本始终执行 release build，并验证实际二进制版本与 capabilities。
`--partial` 只供单机演练；正式汇总禁止缺少任何平台。
`dist/` 由 Git 忽略，不包含素材或私人演示工程。重复演练可使用新的 `--dist` 路径。
安装测试使用临时目录，不修改用户的 PATH、已安装 CLI 或技能。
安装器默认解析 GitHub 最新正式 Release，再固定 tag 下载；回归测试使用本地下载替身覆盖
最新/指定版本、网络失败、无效 tag、平台拒绝及保留旧安装，不依赖仓库已公开发布。

发布附件为三个 `cutcraft-vVERSION-TARGET.tar.gz`、`cutcraft-skill-vVERSION.tar.gz`、
`RELEASE_NOTES.md` 和 `SHA256SUMS`。
CLI 安装脚本独立从仓库 `main` 分支的 `scripts/install.sh` 下载，不再作为 Release 附件发布。
CI 直接验证仓库中的安装脚本；脚本修复可单独合入主分支，无需重新发布二进制。
本地重新 assemble 会移除旧流程生成的 `dist/install.sh`，并重新生成附件校验和。

## 首次发布与后续更新

1. 将源码和 Cargo.lock 提交，设置指向 `XGHeaven/cutcraft` 的 GitHub remote。安装脚本默认使用该仓库，支持 `--repo` 覆盖。
2. 推送分支并确认 CI 全绿；可以先手动运行 Release workflow 检查完整附件。
3. 更新 Cargo.toml 的 CLI 版本及 Cargo.lock，并在 CHANGELOG.md 添加同版本条目。
   项目协议版本独立维护，不因为 CLI 修复版本变化就修改 `SPEC_VERSION`。
4. 将版本更改提交，确保工作区干净；在通过检查的提交上创建 annotated tag：
   `git tag -a v0.1.0 -m 'Cutcraft 0.1.0'`。
5. 准备公开时才推送该 tag：`git push origin v0.1.0`，这一步会触发正式发布。
6. 检查 Release 附件、安装与版本号，再更新对外下载链接。

不要移动已发布 tag 或替换同版本二进制；修复使用新版本。若发布中途失败，先检查
GitHub 是否留下 draft 或部分附件，再处理并重跑；工作流不会自动删除或覆盖发布记录。
SHA-256 是完整性校验，尚未接签名、公证或二进制可复现性保证。
