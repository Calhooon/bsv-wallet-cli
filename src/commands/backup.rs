use anyhow::{anyhow, Result};
use bsv_sdk::primitives::sha256;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn run(db_path: &str, to: Option<PathBuf>) -> Result<()> {
    if !Path::new(".env").exists() {
        return Err(anyhow!(
            ".env not found in current directory; run `bsv-wallet init` first or cd to the wallet dir"
        ));
    }
    if !Path::new(db_path).exists() {
        return Err(anyhow!("{} not found", db_path));
    }

    let out_path = to.unwrap_or_else(|| {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        PathBuf::from(format!("bsv-wallet-backup-{}.tar.gz", ts))
    });

    let mut files: Vec<String> = vec![".env".to_string(), db_path.to_string()];
    let stem = db_path.trim_end_matches(".db");
    for suffix in ["-wal", "-shm"] {
        let p = format!("{}.db{}", stem, suffix);
        if Path::new(&p).exists() {
            files.push(p);
        }
    }

    let status = Command::new("tar")
        .arg("-czf")
        .arg(&out_path)
        .args(&files)
        .status()
        .map_err(|e| anyhow!("failed to spawn tar: {}", e))?;
    if !status.success() {
        return Err(anyhow!("tar exited with status {:?}", status));
    }

    let bytes = std::fs::read(&out_path)?;
    let digest = hex::encode(sha256(&bytes));
    let size_kb = bytes.len() as f64 / 1024.0;

    println!("Backup written: {}", out_path.display());
    println!("Contents: {}", files.join(", "));
    println!("Size: {:.1} KB", size_kb);
    println!("SHA256: {}", digest);
    println!();
    println!("This tarball is plaintext. Encrypt it at rest before storing, e.g.:");
    println!(
        "  age -p -o {}.age {}",
        out_path.display(),
        out_path.display()
    );
    println!("Or store inside an encrypted vault (1Password, encrypted disk image, etc.).");

    Ok(())
}
