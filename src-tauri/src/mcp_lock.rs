//! MCP Server 运行状态记录：lock 文件（PID + 端口 + 启动时间）+ PID 存活检测。
//! GUI 与 httpdeck-mcp 进程共享此文件，实现双向感知（谁先启动都能看到对方）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerInfo {
    pub pid: u32,
    pub port: u16,
    pub endpoint: String,
    pub started_at: String,
}

fn store_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("HttpDeck").join("mcp-server.json")
}

/// 读取当前 MCP server 状态；若 lock 文件存在但 PID 已死，清理后返回 None。
pub fn read_info() -> Option<McpServerInfo> {
    let text = std::fs::read_to_string(store_path()).ok()?;
    let info: McpServerInfo = serde_json::from_str(&text).ok()?;
    if is_process_alive(info.pid) {
        Some(info)
    } else {
        clear_info();
        None
    }
}

pub fn write_info(info: &McpServerInfo) -> Result<(), String> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(info).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn clear_info() {
    let _ = std::fs::remove_file(store_path());
}

pub fn lock_path() -> PathBuf {
    store_path()
}

/// PID 存活检测（跨平台，使用 sysinfo）
/// 注意：只刷新进程信息，不用 System::new_all()——后者会刷新 CPU/磁盘/网络等
/// 全部系统信息，Windows 上走 WMI 耗时数秒，在轮询路径上会卡死 UI。
pub fn is_process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(Pid::from_u32(pid)).is_some()
}

/// 根据 lock 文件里的 PID 终止 MCP server 进程。
pub fn kill_by_pid(pid: u32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            clear_info();
            Ok(())
        } else {
            Err(format!(
                "taskkill 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        let output = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            clear_info();
            Ok(())
        } else {
            Err(format!(
                "kill 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }
}

/// 给定工作区根目录，返回可执行文件 httpdeck-mcp 的路径。
/// 优先与当前 exe 同目录（发布版），其次 target/release，再 target/debug。
pub fn mcp_exe_path() -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "httpdeck-mcp.exe"
    } else {
        "httpdeck-mcp"
    };
    // 与当前 exe 同目录
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 开发模式：workspace 根目录的 target/{release,debug}
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest.parent()?;
    for profile in ["release", "debug"] {
        let candidate = workspace_root
            .join("target")
            .join(profile)
            .join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 两个测试共享同一个全局 lock 文件，必须串行执行，否则并行时会互相覆盖
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn write_and_read_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        let info = McpServerInfo {
            pid: std::process::id(),
            port: 18787,
            endpoint: "http://127.0.0.1:18787/sse".to_string(),
            started_at: "2026-01-01T00:00:00Z".to_string(),
        };
        write_info(&info).unwrap();
        let read = read_info().expect("current process should be alive");
        assert_eq!(read.pid, info.pid);
        assert_eq!(read.port, info.port);
        // 清理
        clear_info();
        assert!(read_info().is_none());
    }

    #[test]
    fn dead_pid_is_cleared() {
        let _guard = TEST_LOCK.lock().unwrap();
        // 找一个几乎肯定不存在的 PID：在高位范围内探测
        let mut dead_pid: Option<u32> = None;
        for candidate in (90000000..100000000u32).step_by(1000) {
            if !is_process_alive(candidate) {
                dead_pid = Some(candidate);
                break;
            }
        }
        let Some(fake_pid) = dead_pid else {
            // 系统上碰巧所有探测的 PID 都存活（极不可能），跳过
            return;
        };
        let info = McpServerInfo {
            pid: fake_pid,
            port: 18787,
            endpoint: String::new(),
            started_at: String::new(),
        };
        write_info(&info).unwrap();
        // read_info 检测到 PID 已死应清理并返回 None
        assert!(
            read_info().is_none(),
            "dead pid should be cleared: {fake_pid}"
        );
    }
}
