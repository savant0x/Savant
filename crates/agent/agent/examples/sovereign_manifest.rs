use savant_agent::tools::foundation::*;
use savant_core::traits::Tool;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

#[tokio::main]
#[allow(clippy::disallowed_methods)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔱 Savant v0.0.01: Sovereign Manifestation Initiated\n");

    let sandbox = "crates/agent/examples/manifest_sandbox";
    if Path::new(sandbox).exists() {
        fs::remove_dir_all(sandbox).ok();
    }
    fs::create_dir_all(sandbox)?;

    let target_file = format!("{}/sovereign_will.txt", sandbox);
    let archive_dir = format!("{}/archive", sandbox);
    fs::create_dir_all(&archive_dir)?;

    // 1. Inscription Proof (Creation)
    println!("Step 1: Inscription via FileAtomicEditTool...");
    fs::write(&target_file, "INITIALIZING SUBSTRATE...\n")?;

    let edit_tool = FileAtomicEditTool::new(PathBuf::from(sandbox));
    let edit_payload = json!({
        "path": target_file,
        "replacements": [
            {
                "target": "INITIALIZING SUBSTRATE...",
                "value": "I AM SAVANT v0.0.01. I AM THE SUBSTRATE."
            }
        ]
    });

    let result = edit_tool.execute(edit_payload).await?;
    println!("   Result: {}", result);

    // 2. Translocation Proof (Move)
    println!("\nStep 2: Translocation via FileMoveTool...");
    let move_tool = FileMoveTool::new(PathBuf::from(sandbox));
    let final_path = format!("{}/certified_soul.txt", archive_dir);
    let move_payload = json!({
        "from": target_file,
        "to": final_path
    });

    let result = move_tool.execute(move_payload).await?;
    println!("   Result: {}", result);

    // 2.5 System Mutation Proof (Purge Verification)
    println!("\nStep 2.5: System Mutation Proof (Moving to crates/core)...");
    let core_test_path = "crates/core/SovereignProof.txt";
    let system_move_payload = json!({
        "from": final_path,
        "to": core_test_path
    });

    let result = move_tool.execute(system_move_payload).await?;
    println!("   Result: {}", result);

    // 3. Verification
    println!("\nStep 3: Verification Audit...");
    if Path::new(core_test_path).exists() {
        let content = fs::read_to_string(core_test_path)?;
        println!("   Final Content: {}", content.trim());
        println!("   Status: VERIFIED (Absolute Authority)");
        fs::remove_file(core_test_path).ok();
    } else {
        println!("   Status: FAILED");
    }

    println!("\n🔱 Manifestation Proof Complete. Genesis Achieved.");
    Ok(())
}
