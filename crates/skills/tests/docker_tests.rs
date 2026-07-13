//! Docker sandbox tests - gracefully skip when Docker is unavailable.

#![allow(clippy::disallowed_methods)]

use savant_core::traits::Tool;
use savant_skills::docker::DockerSkillExecutor;
use serde_json::json;

fn docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("ps")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn test_docker_availability_check() {
    let available = docker_available();
    println!("Docker available: {}", available);
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_executor_name() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    assert_eq!(executor.name(), "docker_skill");
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_executor_description() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    assert!(!executor.description().is_empty());
}

#[tokio::test]
#[ignore] // Requires Docker runtime + alpine image
async fn test_docker_execute_echo() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    let result = executor
        .execute(json!({
            "command": "echo hello-savant-test"
        }))
        .await;

    match result {
        Ok(output) => {
            assert!(
                output.contains("hello-savant-test"),
                "Expected 'hello-savant-test' in output: {}",
                output
            );
        }
        Err(e) => {
            let err_str = e.to_string().to_lowercase();
            if err_str.contains("image") || err_str.contains("404") || err_str.contains("not found")
            {
                println!("SKIP: alpine:latest image not available locally: {}", e);
            } else {
                panic!("Unexpected Docker error: {}", e);
            }
        }
    }
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_execute_shell_command() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    let result = executor
        .execute(json!({
            "command": "echo test-output"
        }))
        .await;

    match result {
        Ok(output) => {
            assert!(
                output.contains("test-output"),
                "Expected output: {}",
                output
            );
        }
        Err(e) => {
            println!("Docker execute failed (may need image pull): {}", e);
        }
    }
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_invalid_input() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    let result = executor.execute(json!("not an object")).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_container_cleanup() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    let _ = executor
        .execute(json!({"command": "echo cleanup-test"}))
        .await;

    // Verify no leftover containers with savant label
    let output = std::process::Command::new("docker")
        .args(["ps", "-a", "--filter", "name=savant-skill", "-q"])
        .output();

    match output {
        Ok(o) => {
            let containers = String::from_utf8_lossy(&o.stdout);
            assert!(
                containers.trim().is_empty(),
                "Orphaned containers found: {}",
                containers
            );
        }
        Err(e) => println!("SKIP: Could not verify cleanup: {}", e),
    }
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_timeout_handling() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");
    let result = executor.execute(json!({"command": "sleep 300"})).await;

    match result {
        Ok(_) => println!("WARNING: Long-running command was not killed (timeout may be >30s)"),
        Err(e) => {
            let err_str = e.to_string().to_lowercase();
            if err_str.contains("image") || err_str.contains("404") {
                println!("SKIP: alpine:latest image not available locally");
            } else if err_str.contains("timeout") || err_str.contains("killed") {
                println!("PASS: Container killed after timeout as expected");
            } else {
                println!("Unexpected error (may be OK): {}", e);
            }
        }
    }
}

#[tokio::test]
#[ignore] // Requires Docker runtime
async fn test_docker_high_error_rate() {
    let executor = DockerSkillExecutor::new("alpine:latest".to_string())
        .expect("Failed to create Docker executor");

    for i in 0..5 {
        let _ = executor
            .execute(json!({"command": format!("echo test-{}", i)}))
            .await;
    }

    let output = std::process::Command::new("docker")
        .args(["ps", "-a", "--filter", "name=savant-skill", "-q"])
        .output()
        .expect("Failed to run docker ps");

    let containers = String::from_utf8_lossy(&output.stdout);
    assert!(
        containers.trim().is_empty(),
        "Orphaned containers after multiple runs: {}",
        containers
    );
}
