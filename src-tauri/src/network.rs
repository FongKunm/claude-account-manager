use crate::{model::*, platform};
use std::{collections::BTreeMap, net::IpAddr, process::Command, time::Duration};

const FABLE: &str = "claude-fable-5-1";
const OPUS: &str = "claude-opus-5-5";
const PREFIX: &str = "anthropic-ratelimit-unified-";
const TRACE_HOSTS: [&str; 2] = ["claude.ai", "api.anthropic.com"];
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}
pub struct Probe {
    pub usage: UsageCheck,
    pub org: Option<String>,
}
pub trait Transport {
    fn trace(&self, proxy: &str, host: &str) -> AppResult<String>;
    fn traces(&self, proxy: &str) -> [AppResult<String>; 2] {
        TRACE_HOSTS.map(|host| self.trace(proxy, host))
    }
    fn message(&self, proxy: &str, token: &str, model: &str) -> AppResult<Response>;
    fn oauth(&self, _proxy: &str, _token: &str, _endpoint: OAuthEndpoint) -> AppResult<Response> {
        Err("此网络实现不支持普通登录查询。".into())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthEndpoint {
    Usage,
    Profile,
}
pub struct Curl;

pub fn validate_proxy(proxy: &str) -> AppResult<String> {
    let url = url::Url::parse(proxy.trim()).map_err(|_| "只支持带端口的本机 HTTP 代理。")?;
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        _ => false,
    };
    if url.scheme() != "http"
        || !local
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("只支持不带密码的本机 HTTP 代理，例如 http://127.0.0.1:7897。".into());
    }
    Ok(url.as_str().trim_end_matches('/').into())
}
pub fn validate_route(proxy: &str, expected: &str) -> AppResult<(String, String)> {
    let proxy = validate_proxy(proxy)?;
    let ip: IpAddr = expected
        .trim()
        .parse()
        .map_err(|_| "请填写正确的固定出口 IP。")?;
    if ip.is_loopback() || ip.is_unspecified() {
        return Err("固定出口不能是本机或未指定地址。".into());
    }
    Ok((proxy, ip.to_string()))
}
fn curl(proxy: &str) -> Command {
    let mut command = Command::new(platform::curl_exe());
    // First argument disables all curlrc files. NO_PROXY is explicitly empty for
    // these checks, with no redirects, direct fallback, custom CA, or shell involved.
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--proxy",
        proxy,
        "--noproxy",
        "",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "8",
        "--max-time",
        "20",
    ]);
    for key in [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "CURL_CA_BUNDLE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        command.env_remove(key).env_remove(key.to_lowercase());
    }
    command
}
impl Transport for Curl {
    fn traces(&self, proxy: &str) -> [AppResult<String>; 2] {
        // Both unauthenticated probes use this round's explicit proxy. Join the
        // worker even if the other domain fails; never reuse a prior result.
        std::thread::scope(|scope| {
            let first = std::thread::Builder::new()
                .spawn_scoped(scope, || self.trace(proxy, TRACE_HOSTS[0]));
            let second = self.trace(proxy, TRACE_HOSTS[1]);
            let first = first
                .map_err(|_| "无法启动出口检查。".to_owned())
                .and_then(|worker| worker.join().map_err(|_| "出口检查意外中止。".to_owned()))
                .and_then(|result| result);
            [first, second]
        })
    }
    fn oauth(&self, _proxy: &str, _token: &str, _endpoint: OAuthEndpoint) -> AppResult<Response> {
        Err("主动额度查询仅通过原生 Claude Code 执行。".into())
    }
    fn trace(&self, proxy: &str, host: &str) -> AppResult<String> {
        #[cfg(feature = "synthetic-tests")]
        crate::lab::check_proxy(proxy)?;
        let url = match host {
            "claude.ai" => "https://claude.ai/cdn-cgi/trace",
            "api.anthropic.com" => "https://api.anthropic.com/cdn-cgi/trace",
            _ => return Err("不允许检查此地址。".into()),
        };
        let mut command = curl(proxy);
        #[cfg(feature = "synthetic-tests")]
        command.arg("--cacert").arg(crate::lab::config()?.ca);
        command.args(["--fail", url]);
        let result = platform::capture(command, Duration::from_secs(24))?;
        if !result.success {
            return Err("出口检查失败，请检查该账号的本机代理。".into());
        }
        String::from_utf8(result.stdout).map_err(|_| "出口检查返回格式不正确。".into())
    }
    fn message(&self, _proxy: &str, _token: &str, _model: &str) -> AppResult<Response> {
        Err("主动额度查询仅通过原生 Claude Code 执行。".into())
    }
}
fn trace_ip(trace: &str) -> AppResult<IpAddr> {
    let ip = trace
        .lines()
        .find_map(|line| line.strip_prefix("ip="))
        .and_then(|v| v.trim().parse::<IpAddr>().ok())
        .ok_or("未能获取官方域名的出口 IP。")?;
    if ip.is_loopback() || ip.is_unspecified() {
        return Err("官方出口检查返回了无效地址。".into());
    }
    Ok(ip)
}
pub fn detect_exit(transport: &impl Transport, proxy: &str) -> AppResult<DetectedExit> {
    let proxy = validate_proxy(proxy)?;
    let [first, second] = transport.traces(&proxy);
    let first = trace_ip(&first?)?;
    let second = trace_ip(&second?)?;
    if first != second {
        return Err("两个地址的出口不一样，代理可能在分流，先别用".into());
    }
    Ok(DetectedExit {
        ip: first.to_string(),
    })
}
pub fn bind_network(transport: &impl Transport, proxy: &str, expected: &str) -> NetworkCheck {
    if !expected.trim().is_empty() {
        return network(transport, proxy, expected);
    }
    match detect_exit(transport, proxy) {
        Ok(result) => NetworkCheck {
            ok: true,
            checked_at: now(),
            ip: Some(result.ip),
            error: None,
        },
        Err(error) => NetworkCheck {
            ok: false,
            checked_at: now(),
            ip: None,
            error: Some(error),
        },
    }
}
#[cfg(test)]
fn parse_response(bytes: &[u8]) -> AppResult<Response> {
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| "用量响应格式不正确。")?;
    let mut remaining = text.as_str();
    let mut status = 0;
    let mut headers = BTreeMap::new();
    // curl can prepend CONNECT and 100-Continue blocks. Keep the final origin block.
    while remaining.starts_with("HTTP/") {
        let end = remaining.find("\r\n\r\n").ok_or("用量响应头不完整。")?;
        let block = &remaining[..end];
        let mut lines = block.lines();
        status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u16>().ok())
            .ok_or("用量响应状态不正确。")?;
        headers.clear();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().into());
            }
        }
        remaining = &remaining[end + 4..];
    }
    if status == 0 {
        return Err("未收到用量响应。".into());
    }
    Ok(Response {
        status,
        headers,
        body: remaining.into(),
    })
}
pub fn network(transport: &impl Transport, proxy: &str, expected: &str) -> NetworkCheck {
    let checked_at = now();
    let result = (|| {
        let (proxy, expected) = validate_route(proxy, expected)?;
        let expected: IpAddr = expected.parse().map_err(|_| "固定出口 IP 不正确。")?;
        for (host, trace) in TRACE_HOSTS.into_iter().zip(transport.traces(&proxy)) {
            let ip = trace_ip(&trace?)?;
            if ip != expected {
                return Err(format!("{host} 出口不一致：预期 {expected}，实测 {ip}。"));
            }
        }
        Ok(expected.to_string())
    })();
    match result {
        Ok(ip) => NetworkCheck {
            ok: true,
            checked_at,
            ip: Some(ip),
            error: None,
        },
        Err(error) => NetworkCheck {
            ok: false,
            checked_at,
            ip: None,
            error: Some(error),
        },
    }
}
fn credits_required(response: &Response) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&response.body) else {
        return false;
    };
    value
        .pointer("/error/details/error_code")
        .and_then(|s| s.as_str())
        == Some("credits_required")
        || (response.status == 429
            && value.pointer("/error/type").and_then(|s| s.as_str()) == Some("rate_limit_error")
            && value
                .pointer("/error/message")
                .and_then(|s| s.as_str())
                .is_some_and(|s| {
                    s.trim()
                        .eq_ignore_ascii_case("Usage credits are required for this model.")
                }))
}
fn buckets(response: &Response) -> Vec<UsageBucket> {
    response
        .headers
        .iter()
        .filter_map(|(key, value)| {
            let key = key.strip_prefix(PREFIX)?.strip_suffix("-utilization")?;
            if key.is_empty()
                || key.len() > 48
                || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return None;
            }
            let fraction: f64 = value.parse().ok()?;
            if !fraction.is_finite() || fraction < 0.0 {
                return None;
            }
            let get = |suffix| response.headers.get(&format!("{PREFIX}{key}-{suffix}"));
            let resets_at = get("reset")
                .and_then(|r| r.parse::<i64>().ok())
                .and_then(|r| chrono::DateTime::from_timestamp(r, 0))
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            let status = match get("status").map(String::as_str) {
                Some("rejected") => "rejected",
                Some("allowed") => "allowed",
                _ if fraction >= 1.0 => "rejected",
                _ => "allowed",
            };
            Some(UsageBucket {
                key: key.into(),
                utilization: (fraction * 100.0).clamp(0.0, 100.0),
                resets_at,
                status: status.into(),
            })
        })
        .collect()
}
pub fn probe(transport: &impl Transport, proxy: &str, token: &str) -> Probe {
    let result = (|| {
        let first = transport.message(proxy, token, FABLE)?;
        // Fallback only for the documented credits_required condition.
        if first.status != 401 && buckets(&first).is_empty() && credits_required(&first) {
            transport.message(proxy, token, OPUS)
        } else {
            Ok(first)
        }
    })();
    let response = match result {
        Ok(r) => r,
        Err(_) => {
            return Probe {
                usage: UsageCheck::error("neterr", "用量请求失败，请检查该账号的代理。"),
                org: None,
            }
        }
    };
    let org = response
        .headers
        .get("anthropic-organization-id")
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        .cloned();
    if response.status == 401 {
        return Probe {
            usage: UsageCheck::error(
                "invalid",
                "令牌无效或已过期，请重新运行 claude setup-token。",
            ),
            org,
        };
    }
    let buckets = buckets(&response);
    if buckets.is_empty() || ![200, 429].contains(&response.status) {
        let error = if response.status == 403 {
            "令牌或套餐无法使用探测模型，请核对账号。"
        } else {
            "未收到可识别的额度数据，请稍后重试。"
        };
        return Probe {
            usage: UsageCheck::error("error", error),
            org,
        };
    }
    let core_limited = buckets
        .iter()
        .any(|b| ["5h", "7d"].contains(&b.key.as_str()) && b.status == "rejected");
    let model_limited = buckets
        .iter()
        .any(|b| !["5h", "7d"].contains(&b.key.as_str()) && b.status == "rejected");
    let globally_rejected = response
        .headers
        .get(&format!("{PREFIX}status"))
        .is_some_and(|s| s == "rejected");
    let state = if core_limited || (globally_rejected && !model_limited) {
        "limited"
    } else {
        "ok"
    };
    let safe_header = |suffix| {
        response
            .headers
            .get(&format!("{PREFIX}{suffix}"))
            .filter(|s| {
                s.len() <= 128
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .cloned()
    };
    let status = safe_header("overage-status");
    let disabled_reason = safe_header("overage-disabled-reason");
    let overage = if status.is_some() || disabled_reason.is_some() {
        Some(Overage {
            status,
            disabled_reason,
        })
    } else {
        None
    };
    Probe {
        org,
        usage: UsageCheck {
            ok: true,
            checked_at: now(),
            state: state.into(),
            error: None,
            buckets: Some(buckets),
            overage,
            source: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Mock {
        responses: RefCell<Vec<Response>>,
        models: RefCell<Vec<String>>,
    }
    impl Transport for Mock {
        fn trace(&self, _: &str, host: &str) -> AppResult<String> {
            Ok(format!(
                "ip={}\n",
                if host == "claude.ai" {
                    "203.0.113.1"
                } else {
                    "203.0.113.2"
                }
            ))
        }
        fn message(&self, _: &str, _: &str, model: &str) -> AppResult<Response> {
            self.models.borrow_mut().push(model.into());
            Ok(self.responses.borrow_mut().remove(0))
        }
    }
    fn response(code: u16, body: &str) -> Response {
        Response {
            status: code,
            body: body.into(),
            headers: BTreeMap::new(),
        }
    }
    fn quota() -> Response {
        let mut r = response(200, "");
        for (key, value) in [
            ("5h-utilization", "0.73"),
            ("5h-reset", "100"),
            ("5h-status", "allowed"),
            ("7d_oi-utilization", "1"),
            ("7d_oi-status", "rejected"),
            ("overage-status", "rejected"),
            ("overage-disabled-reason", "org_level_disabled"),
        ] {
            r.headers.insert(format!("{PREFIX}{key}"), value.into());
        }
        r
    }
    #[test]
    fn checks_both_hosts_and_normalizes_ip() {
        let mock = Mock {
            responses: RefCell::new(vec![]),
            models: RefCell::new(vec![]),
        };
        let result = network(&mock, DEFAULT_PROXY, "203.0.113.1");
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("api.anthropic.com"));
        assert_eq!(
            detect_exit(&mock, DEFAULT_PROXY).err().unwrap(),
            "两个地址的出口不一样，代理可能在分流，先别用"
        );
        assert!(validate_route("http://example.com:8080", "203.0.113.1").is_err());
        assert!(validate_route("http://user:pass@127.0.0.1:8080", "203.0.113.1").is_err());
    }
    #[test]
    fn past_reset_keeps_usage_and_model_limit_is_not_account_limit() {
        let mock = Mock {
            responses: RefCell::new(vec![quota()]),
            models: RefCell::new(vec![]),
        };
        let usage = probe(&mock, DEFAULT_PROXY, "fixture").usage;
        assert_eq!(usage.state, "ok");
        assert_eq!(
            usage.overage.as_ref().unwrap().disabled_reason.as_deref(),
            Some("org_level_disabled")
        );
        let bucket = &usage.buckets.unwrap()[0];
        assert_eq!(bucket.utilization, 73.0);
        assert!(bucket.resets_at.as_ref().unwrap().starts_with("1970"));
    }
    #[test]
    fn falls_back_only_for_credits_required() {
        for (first, expected) in [
            (
                response(
                    403,
                    r#"{"error":{"details":{"error_code":"credits_required"}}}"#,
                ),
                2,
            ),
            (
                response(
                    429,
                    r#"{"error":{"type":"rate_limit_error","message":"Usage credits are required for this model."}}"#,
                ),
                2,
            ),
            (response(401, ""), 1),
            (response(500, ""), 1),
        ] {
            let mock = Mock {
                responses: RefCell::new(vec![first, quota()]),
                models: RefCell::new(vec![]),
            };
            probe(&mock, DEFAULT_PROXY, "fixture");
            assert_eq!(mock.models.borrow().len(), expected);
        }
    }
    #[test]
    fn parses_final_http_headers_and_does_not_use_proxy_headers() {
        let r=parse_response(b"HTTP/1.1 200 Connection established\r\nanthropic-organization-id: wrong\r\n\r\nHTTP/2 401\r\nX-Request: yes\r\n\r\n{}").unwrap();
        assert_eq!(r.status, 401);
        assert!(!r.headers.contains_key("anthropic-organization-id"));
    }
}
