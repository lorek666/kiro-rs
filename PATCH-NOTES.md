# PATCH-NOTES — `prd/billing-integrity`

- 仓库：`https://github.com/lorek666/kiro-rs`（public mirror）
- 基线：`master` = `8fb3dd691d1cedf437bd6a4e9ef9f60b3f217fdd`（与上游 `hank9999/kiro.rs` 同 commit，已 `git diff` 验证为零差异）
- 分支：`prd/billing-integrity`
- 作者身份：`lorek666 <265249269+lorek666@users.noreply.github.com>`
- **`master` 未被改动**；本分支快进式新增，未 force-push。
- 约束遵守：未改 `LICENSE` / `README.md` 的作者声明；未引入任何真实 `config.json` / `credentials.json`；未触碰服务器与部署。

构建方式（本地已实跑，非「应该能编译」）：

```bash
cd admin-ui && pnpm install && pnpm build   # 产出 admin-ui/dist（rust-embed 需要）
cd .. && cargo test
```

`cargo test` 结果：**238 passed; 0 failed; 0 ignored**，编译期 **0 warning / 0 error**；
`cargo clippy --all-targets` 不新增 warning（完整日志见 PR 描述）。

---

## 1. 改了什么

### 1.1 恢复 `meteringEvent` 解析（**唯一的真实功能缺口**）

| 文件 | 改动 |
|---|---|
| `src/kiro/model/events/metering.rs` | **新增**（上游曾存在、后被删除，见 §2.1） |
| `src/kiro/model/events/mod.rs` | 挂载 `mod metering;` + 导出 `MeteringEvent` |
| `src/kiro/model/events/base.rs:74` | `Metering(())` → `Metering(super::MeteringEvent)` |
| `src/kiro/model/events/base.rs:122` | 解析 payload；失败兜底成 `Event::Unknown {}`，不 panic、不向调用方抛错 |

解析器按**两种形状**设计：

```json
{"unit":"credit","unitPlural":"credits","usage":0.324}   // 规范 camelCase
{"usage":0.324}                                           // legacy 裸形状
```

加固点（与上游原始实现 `99f786b^` 的差异，均为有意）：

1. `unit` / `unit_plural` → `Option<String>`、`usage` → `Option<f64>`：
   让「字段缺失」与「字段为 0」可区分（原始实现用 `#[serde(default)]` + 非 Option，
   缺字段会静默变成 `""` / `0.0`）。
2. `usage` 自定义反序列化（`deserialize_optional_f64`）：除 JSON 数字外也接受
   **字符串形式的数字**、缺失、`null`；布尔 / 数组 / 对象一律 `None`。
3. 判定收敛到唯一入口 `MeteringEvent::credits()`：非有限值（NaN/Inf）、负数一律 `None`，
   调用方不需要自己防 NaN。
4. 未知额外字段由 serde 默认忽略，不因上游加字段而整帧报废。

`MeteringEvent` 实现了 `Display`（`"0.324 credits"`），与仓库里仍在但**未编入模块树**的
`src/debug.rs` 期望的形状一致（见 §2.2）。

### 1.2 credit 累计与落盘

| 位置 | 改动 |
|---|---|
| `src/anthropic/stream.rs` | `StreamContext` 新增 `credits_used: f64` + `expose_credits: bool`；`process_kiro_event` 新增 `Event::Metering` 分支累加并打 `tracing::info!` |
| `src/anthropic/handlers.rs` | 非流式事件循环同样累加并打日志 |
| `src/kiro/token_manager.rs:1174` | 新增 `MultiTokenManager::report_credits(credits)` |
| `src/kiro/token_manager.rs` | `CredentialEntry.credits_used`、`StatsEntry.credits_used`（`#[serde(default)]`）、load/save、`CredentialEntrySnapshot.credits_used` |
| `src/kiro/provider.rs:136` | 新增 `KiroProvider::report_credits(credits)` 透传 |
| `src/anthropic/handlers.rs` | 流式（含 `/cc/v1` 缓冲）在流结束分支、非流式在返回前调用 `provider.report_credits(...)` |

落盘复用既有 `success_count` 那条链路（`save_stats_debounced` → `kiro_stats.json`），
所以**重启不丢**。

`StatsEntry.credits_used` 加了 `#[serde(default)]`：旧版本写出的 `kiro_stats.json`
没有这个键，不加默认值会让升级后**整份统计解析失败**（连 `success_count` 一起丢）。
回归测试 `test_stats_entry_backward_compatible_without_credits_used` 覆盖这一点。

**归属口径（有意为之的近似，已在代码注释里写明）**：credit 只有流结束后才可知，
那时请求上下文已不在手上，因此按 `current_id`（本次请求选中的凭据）归属。
并发下若期间发生凭据切换，极少数请求的 credit 会记到相邻凭据。这是**统计口径**近似，
不参与计费决策，也不影响任何限流/禁用判断（那些仍走 `report_success` /
`report_failure` 的精确 `id`）。要做到请求级精确需要把凭据 id 贯穿整条 SSE 流，
代价与收益不成比例。

### 1.3 credit 对外暴露策略（默认不改变客户可见计费输入）

| 文件 | 改动 |
|---|---|
| `src/model/config.rs` | 新增 `expose_credits_in_usage: bool`（JSON 键 `exposeCreditsInUsage`，**默认 `false`**） |
| `src/anthropic/middleware.rs` | `AppState.expose_credits_in_usage` + 链式 `with_expose_credits` |
| `src/anthropic/router.rs` | 新增 `create_router_with_options(..., expose_credits)`；原两参版保留 |
| `src/main.rs` | 传入 `config.expose_credits_in_usage` |
| `src/anthropic/stream.rs` | `message_delta.usage` 逐键构造，开关打开且本 turn 有 metering 时追加 `credits_used` |
| `src/anthropic/handlers.rs` | 非流式 `usage` 同口径追加 |

三条不变量：

1. **默认 `false`** 时 `usage` 的形状与改动前**逐字一致**（不出现 `null` / `0`），
   客户端的计费输入完全不变。
2. **无论开关如何**，credit 都进服务端日志与 `kiro_stats.json`（开关只决定要不要告诉客户端）。
3. **`input_tokens` / `output_tokens` 语义未改动**（`contextUsageEvent` 那条准确值优先逻辑原样保留）。

### 1.4 `402` / 凭据耗尽错误体补强（**只补一处，原重试/禁用逻辑未动**）

| 位置 | 改动 |
|---|---|
| `src/anthropic/handlers.rs` `map_provider_error` | 「所有凭据均已禁用 / 所有凭据已用尽」→ **503 + `upstream_unavailable` + 含可用数的英文 message**（原为 502 `api_error`） |

具体：

- **核查结论（非月度限流的 402）**：`provider.rs` 的 402 分支条件是
  `status == 402 && endpoint.is_monthly_request_limit(&body)`；条件不成立时**不进入**该分支，
  继续往下落到 `if status.is_client_error()` 兜底 → `bail!` 直接返回，
  **既不计入凭据失败、也不禁用**。**不存在误禁用**，因此未做任何改动。
- **仍按要求补了外层映射**：凭据全禁用时 `token_manager` 的三处
  `anyhow::bail!("所有凭据均已禁用（x/y）" / "所有凭据已用尽")` 一路上抛，
  原先被 `map_provider_error` 统一映射成 **502 BAD_GATEWAY** —— 语义上是
  「本网关没有可用凭据」而不是「上游瞬时抖动」，502 会让调用方按可重试处理。
  改为 **503 SERVICE_UNAVAILABLE**，`error.type = "upstream_unavailable"`，
  message 带上可用数。判据用两条中文 bail 文案（它们到达该层时的唯一稳定特征）。
- **未重写** `provider.rs:204/363` 的 402+月度限流禁用逻辑，也未重写 `241/437`
  的 429/408/5xx「只重试不换号」逻辑（按要求）。

---

## 2. 证据

### 2.1 `meteringEvent` 是被「删了又没恢复」的，不是从来没实现过

```
$ git log --oneline --all --diff-filter=D --name-only -- 'src/kiro/model/events/*'
99f786b refactor: 移除无用的 ContextUsageEvent 和 MeteringEvent 事件模型
        src/kiro/model/events/context_usage.rs
        src/kiro/model/events/metering.rs          ← 整个文件被删

$ git log --oneline --all -S "Metering(())" -- src/kiro/model/events/base.rs
99f786b refactor: 移除无用的 ContextUsageEvent 和 MeteringEvent 事件模型
58dc47d 重构后端: domain/infra/service/interface 拆分
```

`99f786b` 的 diff（`base.rs`）：

```diff
-    Metering(super::MeteringEvent),
+    Metering(()),
-    ContextUsage(super::ContextUsageEvent),
+    ContextUsage(()),
-                let payload = super::MeteringEvent::from_frame(&frame)?;
-                Ok(Self::Metering(payload))
+                Ok(Self::Metering(()))
```

紧接着的 `c2bdb0a`「feat: 恢复上下文解析」**只恢复了 `ContextUsageEvent`**：

```
$ git show c2bdb0a --stat
 src/kiro/model/events/base.rs          |  5 +-
 src/kiro/model/events/context_usage.rs | 90 ++++++++++++++++++++++++++++++++++
 src/kiro/model/events/mod.rs           |  2 +
```

`metering.rs` 没被恢复 → `meteringEvent` 帧仍能识别（`EventType::Metering` 一直在），
但 payload 从 2025-12-28 起被静默丢弃至今。本分支按 `99f786b^` 的原始实现恢复。

### 2.2 旁证：仓库里还躺着两个「期望中的 MeteringEvent」引用

`src/debug.rs:138-142`：

```rust
Event::Metering(e) => {
    println!("\n[事件] Metering");
    println!("  unit: {:?}", e.unit);
    println!("  unit_plural: {:?}", e.unit_plural);
    println!("  usage: {}", e.usage);
}
```

这段代码对当前 `Metering(())` **根本编译不过**（`()` 没有 `.unit`）—— 原因是
`src/main.rs` 的模块树只有 `mod admin; mod admin_ui; mod anthropic; mod common;
mod http_client; mod kiro; mod model; pub mod token;`，**`debug.rs` 与 `test.rs` 都不在模块树里**，
是孤儿文件。它们的存在反向证明「`MeteringEvent` 应带 `unit` / `unit_plural` / `usage`」，
本分支恢复的形状与之对齐（并额外实现了 `Display`）。

> 未顺手删除/修复 `debug.rs`：它本身还引用了另一个已被改成 `Unknown {}` 的旧形状，
> 要让它编译通过需要一处与本任务无关的改动；孤儿文件不参与构建，不影响交付。

### 2.3 「`context_input_tokens` 缺失」与「message_delta 没用准确值」—— **PRD 假设不成立**

`src/anthropic/stream.rs:529`（**改动前就存在**）：

```rust
pub context_input_tokens: Option<i32>,
```

`src/anthropic/stream.rs:1179`（**改动前就存在**）：

```rust
let final_input_tokens = self.context_input_tokens.unwrap_or(self.input_tokens);
```

即准确值优先、估算值仅作回退，逻辑早已正确。本分支**没有重写**该实现，
只按 PRD 要求补了测试（`test_metering_event_accumulates_credits` 等围绕 credit；
上下文 token 的既有测试 `test_*context*` 未改动）。

### 2.4 `get_context_window_size` 的 5 倍偏差 —— **PRD 假设不成立**

`src/anthropic/converter.rs:129` `get_context_window_size` 依赖 `map_model`，
而 `map_model`（`converter.rs:80`）用的是 **`contains()` 子串匹配**，不是等值比较：

```rust
if model_lower.contains("opus-5-5") || model_lower.contains("opus-5.5") {
    Some("claude-opus-5.5".to_string())
}
```

因此版本化名、点号名、大小写变体都会命中同一分支。新增表驱动测试
`test_context_window_matches_every_client_spelling` 逐条断言：

| 拼写 | 期望 | 实测 |
|---|---|---|
| `claude-opus-5-5` | 1M | ✅ |
| `claude-opus-5.5` | 1M | ✅ |
| `claude-opus-5-5-20260101` | 1M | ✅ |
| `claude-sonnet-5-5` | 1M | ✅ |
| `claude-sonnet-5-5-20260101` / `claude-opus-5.5-20260101` | 1M | ✅ |
| `claude-opus-5-5-thinking` / `claude-sonnet-5-5-thinking` | 1M | ✅ |
| `Claude-Opus-5-5` / `CLAUDE-SONNET-5-5` | 1M | ✅ |
| `claude-opus-5-5[1m]`（Claude Code 的 1M 标记） | 1M | ✅ |
| 4.6 / 4.7 / 4.8 / 5 / fable 各变体 | 1M | ✅ |
| **反向对照** `claude-sonnet-4-5-*` / `claude-opus-4-5-*` / `claude-haiku-4-5-*` / `claude-3-5-sonnet-*` | 200k | ✅ |
| 未识别模型 | 200k | ✅ |

**结论：不存在回落导致的 5 倍 token 偏差，未改实现。** 反向对照同时证明
4.5 及更老的模型不会被 5.5/5 分支误抬到 1M。

### 2.5 402 的两条分支（改动前现状，已核）

`src/kiro/provider.rs:204` 与 `:363`（两处对称，分别对应 MCP 与 API 路径）：

```rust
if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
    let has_available = self.token_manager.report_quota_exhausted(ctx.id);
    ...
}
```

非月度限流的 402 不满足 `&&`，继续向下走到 `provider.rs:462` 附近的
`if status.is_client_error() { anyhow::bail!(...) }` → **直接返回，不禁用、不计失败**。

---

## 3. 被证伪的 PRD 假设（汇总）

| # | PRD 假设 | 结论 | 依据 |
|---|---|---|---|
| 1 | `src/anthropic/stream.rs` 缺 `context_input_tokens`、message_delta 未用准确值 | **证伪** | `stream.rs:529` / `:1179` 改动前即具备；未重写，只补测试 |
| 2 | `get_context_window_size` 对 `claude-opus-5-5` 等拼写回落 200k，造成 5 倍偏差 | **证伪** | `map_model` 用 `contains()`；新增 23 条正向 + 6 条反向断言全绿 |
| 3 | 非月度限流的 402 会被误禁用 | **证伪** | 条件带 `&&`，落到 `is_client_error()` 兜底 bail；不禁用 |
| 4 | 429/408/5xx 会换号 | **证伪（现状已正确）** | `provider.rs:437-460` 只重试、不动凭据；按要求未重写 |
| 5 | `meteringEvent` 未解析 | **成立（真实缺口）** | `base.rs:71` 原为 `Metering(())`；根因是 `99f786b` 删除 + `c2bdb0a` 只恢复一半 |

---

## 4. 影响面

**对外行为变化（仅一处，且是修 bug）**

| 变化 | 影响 |
|---|---|
| 凭据全耗尽时 HTTP 状态 **502 → 503**，`error.type` `api_error` → `upstream_unavailable`，message 改为英文并带可用数 | 依赖 502 做分支的下游需同步（本仓消费者是 new-api，其把 5xx 一律视为上游不可用并重试，改动后语义更准确、不会误判成上游抖动） |

**默认行为不变的部分**

- `usage` 字段集合默认**完全不变**（`exposeCreditsInUsage=false`）。
- `input_tokens` / `output_tokens` / `stop_reason` / SSE 事件序列不变。
- 凭据选择、重试、禁用策略不变。

**新增的持久化数据**

- `kiro_stats.json` 每个凭据条目多一个 `credits_used` 字段（浮点）。旧版本读到会忽略；
  新版本读旧文件靠 `#[serde(default)]` 兼容。
- `Admin API` 的凭据快照多一个 `creditsUsed` 字段（纯增量）。

**新增的日志**

- 每收到一帧 `meteringEvent` 一条 `tracing::info!`（含 `credits` / `credits_total` / `unit`）。
  上游通常每轮对话 1 帧，量级很小。

**性能**

- metering 解析是一帧一次的小 JSON 反序列化，每次请求 1~2 次，可忽略。
- `report_credits` 走既有的 `save_stats_debounced`，不新增写盘次数。

---

## 5. 测试覆盖（新增 27 个用例）

| 用例 | 覆盖点 |
|---|---|
| `metering::tests::parses_canonical_camel_shape` | 规范 camelCase 形状 |
| `metering::tests::parses_legacy_bare_shape` | legacy `{"usage":X}` 裸形状 |
| `metering::tests::singular_unit_selected_for_one_credit` | usage==1 用单数 unit |
| `metering::tests::integer_usage_is_accepted_as_f64` | 整数 usage |
| `metering::tests::numeric_string_usage_is_accepted` | 字符串数字 usage |
| `metering::tests::missing_usage_yields_none_not_zero` | **缺失 ≠ 0** |
| `metering::tests::null_usage_yields_none` | null |
| `metering::tests::non_numeric_usage_yields_none_without_error` | 布尔/数组/对象不报错 |
| `metering::tests::negative_and_non_finite_usage_are_rejected` | 负数 / NaN / Inf |
| `metering::tests::unknown_extra_fields_are_ignored` | 未知字段忽略 |
| `metering::tests::empty_object_is_all_none` | `{}` |
| `metering::tests::malformed_json_returns_err_not_panic` | 坏 JSON 不 panic |
| `base::tests::test_metering_event_carries_credit_usage` | **缺口回归**（曾经整个丢弃） |
| `base::tests::test_metering_event_legacy_bare_shape` | 帧级 legacy 形状 |
| `base::tests::test_metering_malformed_payload_falls_back_to_unknown` | 解析失败 → `Unknown` |
| `stream::tests::test_metering_event_accumulates_credits` | 多帧累加 |
| `stream::tests::test_credits_hidden_by_default` | 默认不暴露，形状不变 |
| `stream::tests::test_credits_exposed_when_enabled` | 开关打开后出现 `credits_used` |
| `stream::tests::test_credits_absent_when_enabled_but_no_metering` | 开关开但无 metering → 不写 0 |
| `token_manager::tests::test_multi_token_manager_report_credits_accumulates` | 落盘累加 + 非法值忽略 |
| `token_manager::tests::test_stats_entry_backward_compatible_without_credits_used` | 旧 `kiro_stats.json` 兼容 |
| `converter::tests::test_context_window_matches_every_client_spelling` | 模型名映射表（29 条断言） |
| `handlers::tests::test_credential_exhausted_messages_map_to_503` | **R1**：三类「无可用凭据」文案（含 `所有凭据均无法获取有效 Token`）→ 503 + `upstream_unavailable` |
| `handlers::tests::test_is_credential_exhausted_error_predicate` | **R1**：判据纯逻辑，正例 3 类 + 负例 6 条防过宽匹配 |
| `handlers::tests::test_ordinary_upstream_error_stays_502` | **R1**：普通上游错误（500/400/403/流读失败）→ 502 `api_error` 不变 |
| `handlers::tests::test_too_many_requests_keeps_original_mapping` | **R1**：429 → 保持 502 原映射 |
| `handlers::tests::test_bad_request_specials_unaffected` | **R1**：两个 400 特例不被 503 分支波及 |

全量：**238 passed / 0 failed / 0 warning**。

---

## 6. 回滚

改动全部集中在本分支，且**无 schema/磁盘格式的破坏性变更**，回滚成本很低。

**整分支回滚**（推荐）：

```bash
git checkout master
git branch -D prd/billing-integrity          # 本地
git push --delete mine prd/billing-integrity # 远端（如已推送且需要清理）
```

**只回滚某一项**（各项相互独立）：

| 想回滚 | 操作 |
|---|---|
| credit 对外暴露 | 把配置 `exposeCreditsInUsage` 置 `false`（**默认就是 false**，无需改代码） |
| 503 映射 | 删除 `handlers.rs` `map_provider_error` 里 `所有凭据均已禁用` 那段分支，恢复 502 |
| metering 解析 | `git checkout master -- src/kiro/model/events/ src/kiro/token_manager.rs src/kiro/provider.rs`（注意会一并回滚 credit 落盘） |

**运行时无迁移动作**：`kiro_stats.json` 里多出的 `credits_used` 对旧版本是未知字段，
旧版本会忽略；回滚到 `master` 后统计功能正常，只是不再累计 credit。
不需要删任何文件、不需要改任何配置。

---

## 7. 遗留问题 / 未做的事

1. **credit 归属是统计级近似**（按 `current_id`），并发切换时可能偏移到相邻凭据。
   请求级精确归属需要把凭据 id 贯穿整条 SSE 流，本次未做（见 §1.2）。
2. **`src/debug.rs` / `src/test.rs` 是孤儿文件**（不在 `main.rs` 模块树里），
   其中 `debug.rs` 仍引用已被 `99f786b` 改掉的旧 `Unknown { event_type, payload }` 形状，
   无法直接编译。本次未清理（与本任务无关，且清理会扩大 diff）。
3. **`meteringEvent` 的 credit 单位未做归一**：只按上游下发的 `unit` / `unitPlural` 透传，
   未把 `credit` 换成美元或 token。换算比率不属于本仓库的职责范围。
4. **`exposeCreditsInUsage=true` 时未同步更新 `/v1/models` 或 admin-ui 的展示** ——
   开关只是让字段出现在 `usage` 里，界面不展示 credit。
5. **未做端到端联调**：本次只跑单测（无真实 Kiro 凭据，也不应引入）。
   `meteringEvent` 的真实 payload 形状取自任务给定样例与上游原始实现的字段名，
   若上游实际编码有出入，`Option` + 容错反序列化会退化为「不计数」而不是报错。
---

## 7.x 已知未验证项（N6–N8）

上一节 1–5 是「未做的事」；这三条是**已实现、但前提未经验证**的点，性质不同，
单独列出以免被当成「已验证的行为」。三条都**不影响计费正确性**，属观测/运维风险。

### N6 — `meteringEvent.usage` 是增量还是累计：**未验证**

- **现状**：`stream.rs` / `handlers.rs` 对每帧做 `credits_used += credits`，
  隐含假设「一帧 = 本次增量」。
- **为什么没有依据**：上游从未定义过这个语义。
  `99f786b^` 的原始 `metering.rs` 只实现了 `Display`（打印用），没有消费 `usage`；
  原始 `Event::Metering` 在事件循环里落到 `_ =>` 兜底分支，**从未被消费**。
  也就是说「一帧代表什么」在上游历史上**没有任何参考实现**，我们的累加是假设。
  当前无可用真实凭据，无法用真实帧校验。
- **若假设错误（实为累计值）的后果**：`+=` 会重复计数，credit 统计**高估**。
  影响面仅限观测数据（`kiro_stats.json` 的 `credits_used` 与日志），
  **不参与计费、不限流、不影响禁用判定**。
- **如何判定（无需改代码）**：`tracing::info!` 已同时打印 `credits`（本帧）与
  `credits_total`（累加）。抓同一次请求的多帧日志：
  - 各帧 `credits` 小幅新增、`credits_total` 单调累加到真实消耗 → **增量，现有实现正确**；
  - 各帧 `credits` 本身单调递增且末帧 ≈ 真实消耗（`credits_total` 明显偏大）→ **累计**，
    需要把 `+=` 改成 `max` / 覆盖赋值。
- **修法位置**：`src/anthropic/stream.rs` `Event::Metering` 分支、
  `src/anthropic/handlers.rs` 非流式同名分支（两处，改动量各 1 行）。
  详见 `src/kiro/model/events/metering.rs` 模块头「未知与风险」一节。

### N7 — 503 判据是基于**文案**的脆弱匹配

- **现状**：`handlers.rs::is_credential_exhausted_error` 用中文字符串片段
  （`所有凭据均已禁用` / `所有凭据已用尽` / `所有凭据均无法获取有效 Token`）
  判定「无可用凭据」，命中回 503。
- **为什么脆**：`token_manager` / `provider` 两层都是 `anyhow::bail!` 纯字符串上抛，
  没有可 `downcast` 的类型。**改那几处 bail! 文案必须同步改本函数**，
  否则该分支静默失效 —— 表现为回退成 502，调用方重新把它当上游抖动去重试。
- **已缓解**：判据集中到单一函数，并把三类文案的 `file:line` 列在注释里；
  单测 `test_credential_exhausted_messages_map_to_503` 钉住三类文案，
  `test_is_credential_exhausted_error_predicate` 含负例防过宽匹配。
- **根治方向**：把「无可用凭据」提成具名错误类型（如 `NoAvailableCredentialError`），
  调用方 `downcast_ref` 判定。本次为最小改动未做。

### N8 — credit 归属口径是统计级近似

- **现状**：`token_manager.rs::report_credits` 按 `*self.current_id.lock()` 归属。
- **不会死锁**：该 guard 是**语句内临时量**（语句结束即 drop），不跨语句存活，
  与仓库内「先 `entries` 后 `current_id`」的常规顺序**不会反向**，无 ABBA 条件。
- **但仍是两个独立临界区**：读 `current_id` 与锁 `entries` 之间若发生凭据切换，
  本次 credit 会记到切换后的条目上。
- **影响**：仅统计归属可能偏移到相邻凭据；不影响计费/限流/禁用判定
  （那些走 `report_success` / `report_failure` 的精确 `id`）。
- **修法**：若将来需要请求级精确，需把凭据 id 从 `acquire_context` 一路贯穿到
  SSE 流结束（改动面较大，本次未做）。
