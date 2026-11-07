# 镜像说明 / Mirror notice

本仓库是 [hank9999/kiro.rs](https://github.com/hank9999/kiro.rs) 的**公开镜像**，
用于固档我们在生产环境实际运行的版本，并便于日后向上游提交 issue / PR。

- **上游（唯一权威来源）**：https://github.com/hank9999/kiro.rs
- **原作者**：hank9999 · **许可**：MIT（见 [LICENSE](LICENSE)，© 2026 hank9999）
- **本仓库不是上游官方仓库**，不承诺比上游更新，也不代表作者立场。

## 当前固档版本

| 项 | 值 |
|---|---|
| 上游 commit | `8fb3dd691d1cedf437bd6a4e9ef9f60b3f217fdd`（2026-10-29） |
| 上游版本号 | `2026.10.1` |
| 对应生产镜像 | `ghcr.io/hank9999/kiro-rs@sha256:d4e95526…e6259`（= tag `beta-8fb3dd`） |
| 上游分支 | `master`（另有 `refactor/v2`） |

> 生产环境用 digest 固定镜像，不跟随 `latest`（ghcr 的 `latest` 停在 2026-03-30 的 `2026.3.1`，落后约 7 个月）。

## 同步上游

```bash
git remote add upstream https://github.com/hank9999/kiro.rs.git   # 只需一次
git fetch upstream --prune --tags
git push origin \
  refs/remotes/upstream/master:refs/heads/master \
  refs/remotes/upstream/refactor/v2:refs/heads/refactor/v2 \
  --tags
```

## 边界

- 本仓库**只含上游源码**；运行时的 `config.json`、`credentials.json`、余额缓存等
  一律不入库（含密钥，只存在于服务器 `/opt/ai/kiro-rs/config/`）。
- 升级前**先审 diff 再升**，并核对镜像 tag ↔ commit 是否对得上。
- 若上游出现与生产不一致的行为，以**我们固档的这个 commit** 为准排查。

---
This repository is a public mirror of `hank9999/kiro.rs` (MIT), pinned to commit
`8fb3dd6` (v2026.10.1) — the exact build we run in production. Upstream remains
the source of truth.
