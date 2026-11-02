use crate::domain::ExecutionResult;
use anyhow::{Context, Result, bail};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub const CAPTURE_LIMIT: usize = 512 * 1024;
pub fn command(exe: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW; installer UAC remains under Windows control.
    cmd
}

async fn capture(reader: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    let mut reader = reader;
    let mut out = Vec::new();
    let mut buf = [0; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        if n > CAPTURE_LIMIT.saturating_sub(out.len()) {
            truncated = true;
        }
        if out.len() < CAPTURE_LIMIT {
            out.extend_from_slice(&buf[..n.min(CAPTURE_LIMIT - out.len())]);
        }
    }
    Ok((out, truncated))
}

pub async fn run(
    exe: impl AsRef<std::ffi::OsStr>,
    args: &[String],
    read_only: bool,
) -> Result<ExecutionResult> {
    let mut cmd = command(exe);
    // Only read-only commands may be killed. Never terminate an installer mid-transaction.
    cmd.kill_on_drop(read_only);
    let mut child = cmd
        .args(args)
        .spawn()
        .context("Não foi possível iniciar o processo")?;
    let stdout = tokio::spawn(capture(child.stdout.take().context("stdout indisponível")?));
    let stderr = tokio::spawn(capture(child.stderr.take().context("stderr indisponível")?));
    let status = if read_only {
        match tokio::time::timeout(Duration::from_secs(120), child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                let _ = child.kill().await;
                stdout.abort();
                stderr.abort();
                bail!(
                    "O comando excedeu 120 segundos. Verifique a rede e a disponibilidade do Winget."
                );
            }
        }
    } else {
        child.wait().await?
    };
    let (out, out_truncated) = stdout.await??;
    let (err, err_truncated) = stderr.await??;
    if read_only && (out_truncated || err_truncated) {
        bail!(
            "A saída excedeu o limite seguro de 512 KiB. Refine a consulta; nenhum resultado parcial foi usado."
        );
    }
    Ok(ExecutionResult {
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        exit_code: status.code(),
        ..Default::default()
    })
}

pub fn open_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("O arquivo ou diretório não existe mais");
    }
    shell_open(path.as_os_str())
}
pub fn open_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https" && parsed.scheme() != "ms-settings" {
        bail!("Protocolo não permitido");
    }
    shell_open(std::ffi::OsStr::new(url))
}
#[cfg(windows)]
fn shell_open(target: &std::ffi::OsStr) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        bail!(
            "O Windows não conseguiu abrir o recurso (código {})",
            result as isize
        );
    }
    Ok(())
}
#[cfg(not(windows))]
fn shell_open(_: &std::ffi::OsStr) -> Result<()> {
    bail!("Recurso disponível apenas no Windows")
}
