//! 事件基础定义
//!
//! 定义事件类型枚举、trait 和统一事件结构

use crate::kiro::parser::error::{ParseError, ParseResult};
use crate::kiro::parser::frame::Frame;

/// 事件类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    /// 助手响应事件
    AssistantResponse,
    /// 工具使用事件
    ToolUse,
    /// 计费事件
    Metering,
    /// 上下文使用率事件
    ContextUsage,
    /// 未知事件类型
    Unknown,
}

impl EventType {
    /// 从事件类型字符串解析
    pub fn from_str(s: &str) -> Self {
        match s {
            "assistantResponseEvent" => Self::AssistantResponse,
            "toolUseEvent" => Self::ToolUse,
            "meteringEvent" => Self::Metering,
            "contextUsageEvent" => Self::ContextUsage,
            _ => Self::Unknown,
        }
    }

    /// 转换为事件类型字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AssistantResponse => "assistantResponseEvent",
            Self::ToolUse => "toolUseEvent",
            Self::Metering => "meteringEvent",
            Self::ContextUsage => "contextUsageEvent",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 事件 payload trait
///
/// 所有具体事件类型都需要实现此 trait
pub trait EventPayload: Sized {
    /// 从帧解析事件负载
    fn from_frame(frame: &Frame) -> ParseResult<Self>;
}

/// 统一事件枚举
///
/// 封装所有可能的事件类型
#[derive(Debug, Clone)]
pub enum Event {
    /// 助手响应
    AssistantResponse(super::AssistantResponseEvent),
    /// 工具使用
    ToolUse(super::ToolUseEvent),
    /// 计费
    ///
    /// 曾经是 `Metering(())`（payload 被整个丢弃，见 `metering.rs` 模块头），
    /// 现在恢复为携带实际计费信息。
    Metering(super::MeteringEvent),
    /// 上下文使用率
    ContextUsage(super::ContextUsageEvent),
    /// 未知事件 (保留原始帧数据)
    Unknown {},
    /// 服务端错误
    Error {
        /// 错误代码
        error_code: String,
        /// 错误消息
        error_message: String,
    },
    /// 服务端异常
    Exception {
        /// 异常类型
        exception_type: String,
        /// 异常消息
        message: String,
    },
}

impl Event {
    /// 从帧解析事件
    pub fn from_frame(frame: Frame) -> ParseResult<Self> {
        let message_type = frame.message_type().unwrap_or("event");

        match message_type {
            "event" => Self::parse_event(frame),
            "error" => Self::parse_error(frame),
            "exception" => Self::parse_exception(frame),
            other => Err(ParseError::InvalidMessageType(other.to_string())),
        }
    }

    /// 解析事件类型消息
    fn parse_event(frame: Frame) -> ParseResult<Self> {
        let event_type_str = frame.event_type().unwrap_or("unknown");
        let event_type = EventType::from_str(event_type_str);

        match event_type {
            EventType::AssistantResponse => {
                let payload = super::AssistantResponseEvent::from_frame(&frame)?;
                Ok(Self::AssistantResponse(payload))
            }
            EventType::ToolUse => {
                let payload = super::ToolUseEvent::from_frame(&frame)?;
                Ok(Self::ToolUse(payload))
            }
            EventType::Metering => {
                // 计费事件的 payload 只用于计量与观测：解析失败**不得**打断事件流，
                // 也不得让调用方拿到半条错误。兜底成 Unknown（与未知事件同一条路）。
                match super::MeteringEvent::from_frame(&frame) {
                    Ok(payload) => Ok(Self::Metering(payload)),
                    Err(_) => Ok(Self::Unknown {}),
                }
            }
            EventType::ContextUsage => {
                let payload = super::ContextUsageEvent::from_frame(&frame)?;
                Ok(Self::ContextUsage(payload))
            }
            EventType::Unknown => Ok(Self::Unknown {}),
        }
    }

    /// 解析错误类型消息
    fn parse_error(frame: Frame) -> ParseResult<Self> {
        let error_code = frame
            .headers
            .error_code()
            .unwrap_or("UnknownError")
            .to_string();
        let error_message = frame.payload_as_str();

        Ok(Self::Error {
            error_code,
            error_message,
        })
    }

    /// 解析异常类型消息
    fn parse_exception(frame: Frame) -> ParseResult<Self> {
        let exception_type = frame
            .headers
            .exception_type()
            .unwrap_or("UnknownException")
            .to_string();
        let message = frame.payload_as_str();

        Ok(Self::Exception {
            exception_type,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_from_str() {
        assert_eq!(
            EventType::from_str("assistantResponseEvent"),
            EventType::AssistantResponse
        );
        assert_eq!(EventType::from_str("toolUseEvent"), EventType::ToolUse);
        assert_eq!(EventType::from_str("meteringEvent"), EventType::Metering);
        assert_eq!(
            EventType::from_str("contextUsageEvent"),
            EventType::ContextUsage
        );
        assert_eq!(EventType::from_str("unknown_type"), EventType::Unknown);
    }

    #[test]
    fn test_event_type_as_str() {
        assert_eq!(
            EventType::AssistantResponse.as_str(),
            "assistantResponseEvent"
        );
        assert_eq!(EventType::ToolUse.as_str(), "toolUseEvent");
    }

    /// 构造一个 `event` 类消息帧（只填 metering 解析需要的 headers + payload）
    fn metering_frame(payload: &str) -> Frame {
        let mut headers = crate::kiro::parser::header::Headers::new();
        headers.insert(
            ":message-type".to_string(),
            crate::kiro::parser::header::HeaderValue::String("event".to_string()),
        );
        headers.insert(
            ":event-type".to_string(),
            crate::kiro::parser::header::HeaderValue::String("meteringEvent".to_string()),
        );
        Frame {
            headers,
            payload: payload.as_bytes().to_vec(),
        }
    }

    #[test]
    fn test_metering_event_carries_credit_usage() {
        // 缺口回归：这个 payload 以前被 `Metering(())` 整个丢掉
        let event = Event::from_frame(metering_frame(
            r#"{"unit":"credit","unitPlural":"credits","usage":0.324}"#,
        ))
        .expect("meteringEvent 应可解析");
        match event {
            Event::Metering(m) => assert_eq!(m.credits(), Some(0.324)),
            other => panic!("应解析为 Metering，实际: {other:?}"),
        }
    }

    #[test]
    fn test_metering_event_legacy_bare_shape() {
        let event = Event::from_frame(metering_frame(r#"{"usage":2.5}"#))
            .expect("legacy 裸形状应可解析");
        match event {
            Event::Metering(m) => {
                assert_eq!(m.credits(), Some(2.5));
                // unit 缺失时不应变成空串，而是 None
                assert_eq!(m.unit, None);
            }
            other => panic!("应解析为 Metering，实际: {other:?}"),
        }
    }

    #[test]
    fn test_metering_malformed_payload_falls_back_to_unknown() {
        // 非 JSON 不得 panic、不得把错误抛给调用方 —— 走 Unknown
        let event = Event::from_frame(metering_frame("not-json"))
            .expect("metering 解析失败不应向上抛错");
        assert!(matches!(event, Event::Unknown {}));
    }
}
