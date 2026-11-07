//! 计费事件
//!
//! 处理 meteringEvent 类型的事件。
//!
//! ## 这个文件为什么要恢复
//!
//! 上游提交 `99f786b`（refactor: 移除无用的 ContextUsageEvent 和 MeteringEvent 事件模型）
//! 把本文件整个删除，同时把 `Event::Metering(super::MeteringEvent)` 降级成
//! `Event::Metering(())`；紧接着的 `c2bdb0a`（feat: 恢复上下文解析）只把
//! `ContextUsageEvent` 恢复了，`MeteringEvent` **没有跟着恢复**。
//!
//! 结果是：`meteringEvent` 帧仍然能被识别（`EventType::Metering` 一直在），
//! 但 payload 从那一刻起被整体丢弃 —— 上游下发的 credit 用量在网关里不可见。
//! 本文件按 `99f786b^` 的原始实现恢复，并做了三处加固：
//!
//! 1. `unit` / `unit_plural` 改为 `Option<String>`、`usage` 改为 `Option<f64>`，
//!    让「字段缺失」与「字段为 0」可区分（原始实现用 `#[serde(default)]` +
//!    非 Option 类型，缺字段会静默变成 `""` / `0.0`，无法判断到底有没有用量）；
//! 2. `usage` 自定义反序列化：除 JSON 数字外也接受字符串形式的数字
//!    （上游换编码时不会整帧解析失败）；
//! 3. 缺失 / null / 非数值 / NaN / Inf / 负数一律收敛成 `None`，
//!    判定入口是 [`MeteringEvent::credits`]，调用方永远不需要自己防 NaN。
//!
//! 兼容的两种形状：
//!
//! ```json
//! {"unit": "credit", "unitPlural": "credits", "usage": 0.324}
//! {"usage": 0.324}
//! ```
//!
//! 未知的额外字段由 serde 默认忽略（未开 `deny_unknown_fields`），不会因此报错。
//!
//! ## ⚠️ 未知与风险：`usage` 是**增量**还是**累计**，尚未验证
//!
//! 本仓库对 `usage` 的语义**没有任何上游参考**：
//!
//! - `99f786b^` 的原始 `metering.rs` 只实现了 `Display`（格式化打印），
//!   并没有消费过 `usage` 做任何累加；
//! - 原始的 `Event::Metering` 在 `stream.rs` / `handlers.rs` 的事件循环里
//!   落到 `_ =>` 兜底分支，**从未被消费**。当时的代码是
//!   `Event::Metering(_) => ...`，因此原始版本也就没有给出「一帧代表什么」的定义。
//!
//! 也就是说：**「一帧 `usage` 是本次增量，还是到目前为止的累计值」没有任何依据**，
//! 本分支在 `stream.rs` / `handlers.rs` 里做的
//!
//! ```ignore
//! self.credits_used += credits;   // 隐含假设：每帧是增量
//! ```
//!
//! 是**一个假设**，不是已验证的事实。当前无可用真实凭据，无法用真实帧校验。
//!
//! ### 风险：若 `usage` 其实是累计值
//!
//! 现有的 `+=` 会**重复计数**（把 0.324 + 0.648 + 0.972 … 全部加起来），
//! credit 统计会显著高估。注意影响范围：这**只影响观测统计**
//! （`kiro_stats.json` 的 `credits_used` 与日志），**不参与计费、不限流、
//! 不影响任何禁用判定**；即使高估也不会让真实计费出错。
//!
//! ### 如何用真实流量判定（无需改代码）
//!
//! `stream.rs` / `handlers.rs` 命中 meteringEvent 时打的 `tracing::info!`
//! 同时输出两个字段：
//!
//! - `credits` —— **本帧**的值；
//! - `credits_total` —— 累加后的值。
//!
//! 判定方法：抓同一次请求的多帧日志，
//!
//! - 若各帧 `credits` 是**小幅新增**、`credits_total` 单调累加到末帧的值
//!   ≈ 本次请求真实消耗 → 增量语义，**现有实现正确**；
//! - 若各帧 `credits` **本身就单调递增**且末帧 `credits` 就等于真实消耗
//!   （即 `credits_total` 明显大于末帧 `credits`）→ **累计语义**，
//!   需要把 `+=` 改成 `max`/覆盖赋值。
//!
//! 该取舍与验证方法已记入 `PATCH-NOTES.md` §7（N6）。

use serde::{Deserialize, Deserializer};

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// 计费事件
///
/// 包含本次请求的资源使用计费信息
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeteringEvent {
    /// 计费单位（单数形式），如 `credit`
    #[serde(default)]
    pub unit: Option<String>,
    /// 计费单位（复数形式），如 `credits`
    #[serde(default)]
    pub unit_plural: Option<String>,
    /// 使用量（单位见 `unit` / `unit_plural`）
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub usage: Option<f64>,
}

/// 宽松地把 JSON 值读成 `Option<f64>`
///
/// - 缺失 / `null` / 布尔 / 数组 / 对象 → `None`（不做类型硬失败）
/// - 数字 → 原值
/// - 字符串 → `trim` 后按浮点解析，失败同样 `None`
///
/// 这么做的理由：meteringEvent 只用于计量与观测，**任何**一帧的编码抖动都不该
/// 让整条事件流解析失败（更不该 panic）。读不出来就是「这次没读到 credit」。
fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
        Some(_) => None,
    })
}

impl EventPayload for MeteringEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

impl MeteringEvent {
    /// 本次事件计到的 credit 数
    ///
    /// 只有「有限且非负」的数值才算数：`NaN` / `Inf` / 负数一律视为「没读到」，
    /// 返回 `None`。调用方据此决定要不要累加，**不需要**自己再做一次校验。
    pub fn credits(&self) -> Option<f64> {
        self.usage.filter(|value| value.is_finite() && *value >= 0.0)
    }

    /// 计费单位：按用量是否为 1 在单数 / 复数之间选择；都缺失时兜底 `credit`
    pub fn unit_label(&self) -> &str {
        let preferred = match self.credits() {
            Some(usage) if usage == 1.0 => self.unit.as_deref(),
            _ => self.unit_plural.as_deref(),
        };
        preferred
            .filter(|text| !text.trim().is_empty())
            .or(self.unit.as_deref())
            .unwrap_or("credit")
    }

    /// 获取格式化的简短使用量字符串 (3 位小数精度)
    pub fn formatted_usage_short(&self) -> String {
        self.format_usage_with_precision(3)
    }

    /// 使用指定精度格式化使用量；用量未知时返回 `unknown`
    fn format_usage_with_precision(&self, precision: usize) -> String {
        match self.credits() {
            Some(usage) => format!(
                "{:.prec$} {}",
                usage,
                self.unit_label(),
                prec = precision
            ),
            None => "unknown".to_string(),
        }
    }
}

impl std::fmt::Display for MeteringEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.formatted_usage_short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用裸 payload 构造 Frame（metering 解析只读 payload，不看 headers）
    fn frame_with(payload: &str) -> Frame {
        Frame {
            headers: crate::kiro::parser::header::Headers::default(),
            payload: payload.as_bytes().to_vec(),
        }
    }

    fn parse(payload: &str) -> MeteringEvent {
        MeteringEvent::from_frame(&frame_with(payload)).expect("metering payload 应可解析")
    }

    #[test]
    fn parses_canonical_camel_shape() {
        let event = parse(r#"{"unit":"credit","unitPlural":"credits","usage":0.324}"#);
        assert_eq!(event.unit.as_deref(), Some("credit"));
        assert_eq!(event.unit_plural.as_deref(), Some("credits"));
        assert_eq!(event.credits(), Some(0.324));
        assert_eq!(event.unit_label(), "credits");
        assert_eq!(event.formatted_usage_short(), "0.324 credits");
    }

    #[test]
    fn parses_legacy_bare_shape() {
        // 只有 usage，没有 unit / unitPlural —— 旧版 / 精简编码
        let event = parse(r#"{"usage":1.5}"#);
        assert_eq!(event.unit, None);
        assert_eq!(event.unit_plural, None);
        assert_eq!(event.credits(), Some(1.5));
        // 单位缺失时兜底 credit
        assert_eq!(event.unit_label(), "credit");
        assert_eq!(event.formatted_usage_short(), "1.500 credit");
    }

    #[test]
    fn singular_unit_selected_for_one_credit() {
        let event = parse(r#"{"unit":"credit","unitPlural":"credits","usage":1}"#);
        assert_eq!(event.credits(), Some(1.0));
        assert_eq!(event.unit_label(), "credit");
    }

    #[test]
    fn integer_usage_is_accepted_as_f64() {
        let event = parse(r#"{"unit":"credit","unitPlural":"credits","usage":7}"#);
        assert_eq!(event.credits(), Some(7.0));
    }

    #[test]
    fn numeric_string_usage_is_accepted() {
        // 上游把 usage 换成字符串时不至于整帧报废
        let event = parse(r#"{"usage":"0.25"}"#);
        assert_eq!(event.credits(), Some(0.25));
    }

    #[test]
    fn missing_usage_yields_none_not_zero() {
        // 关键语义：缺失 != 0
        let event = parse(r#"{"unit":"credit","unitPlural":"credits"}"#);
        assert_eq!(event.usage, None);
        assert_eq!(event.credits(), None);
    }

    #[test]
    fn null_usage_yields_none() {
        let event = parse(r#"{"usage":null}"#);
        assert_eq!(event.credits(), None);
    }

    #[test]
    fn non_numeric_usage_yields_none_without_error() {
        let event = parse(r#"{"usage":"not-a-number"}"#);
        assert_eq!(event.credits(), None);
        // 布尔 / 数组 / 对象同样不报错
        assert_eq!(parse(r#"{"usage":true}"#).credits(), None);
        assert_eq!(parse(r#"{"usage":[1]}"#).credits(), None);
        assert_eq!(parse(r#"{"usage":{"v":1}}"#).credits(), None);
    }

    #[test]
    fn negative_and_non_finite_usage_are_rejected() {
        // 负数不是合法用量；NaN/Inf 不是合法 JSON 数字，用字符串形态兜底验证
        assert_eq!(parse(r#"{"usage":-1.0}"#).credits(), None);
        assert_eq!(parse(r#"{"usage":"NaN"}"#).credits(), None);
        assert_eq!(parse(r#"{"usage":"inf"}"#).credits(), None);
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let event = parse(r#"{"usage":0.5,"somethingNew":{"a":1},"unit":"credit"}"#);
        assert_eq!(event.credits(), Some(0.5));
        assert_eq!(event.unit.as_deref(), Some("credit"));
    }

    #[test]
    fn empty_object_is_all_none() {
        let event = parse("{}");
        assert_eq!(event.credits(), None);
        assert_eq!(event.unit, None);
        assert_eq!(event.unit_plural, None);
    }

    #[test]
    fn malformed_json_returns_err_not_panic() {
        // 上层会把 Err 兜成 Event::Unknown，这里只保证「不 panic」
        assert!(MeteringEvent::from_frame(&frame_with("not json at all")).is_err());
        assert!(MeteringEvent::from_frame(&frame_with("")).is_err());
    }
}