//! 测试辅助库：配置样例、fixture 服务定义

use cli_companion_domain::ServiceDefinition;
use std::path::PathBuf;

/// 生成一个用于测试的服务定义（Windows 上用 cmd/echo 类程序）
pub fn test_service(name: &str) -> ServiceDefinition {
    ServiceDefinition::new(name, PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
}

/// 开发文档 §12.2 示例配置（回归测试用）
pub const SAMPLE_SERVICES_JSON: &str = include_str!("../fixtures/services.sample.json");
