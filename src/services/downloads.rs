use crate::{domain::*, providers::ExecuteContext};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub const MAX_DOWNLOAD_SIZE: u64 = 4 * 1024 * 1024 * 1024;
pub struct DownloadSpec {
    pub url: String,
    pub filename: String,
    pub namespace: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub version: Option<String>,
    pub release_url: Option<String>,
    pub origin: String,
}
pub fn verify_hash(actual: &str, expected: Option<&str>) -> Result<bool> {
    if let Some(expected) = expected {
        validate_sha256(expected)?;
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("SHA-256 divergente. O arquivo foi bloqueado e descartado.");
        }
        return Ok(true);
    }
    Ok(false)
}
pub fn verify_size(received: u64, expected: Option<u64>) -> Result<()> {
    if received == 0 {
        bail!("Resposta vazia: nenhum instalador foi recebido");
    }
    if received > MAX_DOWNLOAD_SIZE || expected.is_some_and(|n| n != received) {
        bail!("Tamanho do download divergente ou acima de 4 GB: {received} bytes");
    }
    Ok(())
}
pub fn direct_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .user_agent(concat!("Winit/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 10 {
                return a.error("Redirecionamentos excessivos");
            }
            if validate_https(a.url().as_str()).is_ok() {
                a.follow()
            } else {
                a.error("Redirecionamento deve permanecer em HTTPS, sem credenciais")
            }
        }))
        .build()?)
}
fn public_url(url: &reqwest::Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}
pub async fn download(
    client: &reqwest::Client,
    spec: DownloadSpec,
    ctx: ExecuteContext,
) -> Result<ExecutionResult> {
    let requested = validate_https(&spec.url)?;
    if ctx.cancel.load(Ordering::Relaxed) {
        bail!("Download cancelado pelo usuário");
    }
    if !safe_filename(&spec.filename)
        || !download_extension(&spec.filename)
        || !safe_filename(&spec.namespace)
    {
        bail!("Nome de arquivo ou diretório de download inválido");
    }
    if let Some(hash) = &spec.sha256 {
        validate_sha256(hash)?;
    }
    if spec.size.is_some_and(|s| s > MAX_DOWNLOAD_SIZE) {
        bail!("Asset excede o limite de 4 GB");
    }
    let folder = ctx.downloads.join(&spec.namespace);
    tokio::fs::create_dir_all(&folder).await?;
    let destination = folder.join(&spec.filename);
    if destination.exists() {
        if let Some(expected) = spec.sha256.as_deref() {
            let mut existing = tokio::fs::File::open(&destination).await?;
            use tokio::io::AsyncReadExt;
            let mut hash = Sha256::new();
            let mut bytes = 0_u64;
            let mut buffer = [0; 65536];
            (ctx.progress)(Progress {
                message: "Conferindo SHA-256 do arquivo existente; sem novo download".into(),
                ..Default::default()
            });
            loop {
                if ctx.cancel.load(Ordering::Relaxed) {
                    bail!("Verificação cancelada pelo usuário");
                }
                let n = existing.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                if bytes > MAX_DOWNLOAD_SIZE {
                    bail!("Arquivo existente excede 4 GB");
                }
                hash.update(&buffer[..n]);
            }
            drop(existing);
            verify_size(bytes, spec.size)?;
            let digest = hex::encode(hash.finalize());
            if !digest.eq_ignore_ascii_case(expected) {
                bail!(
                    "O arquivo existente difere do SHA-256 esperado e foi preservado. Renomeie ou remova-o antes de baixar novamente."
                );
            }
            write_zone_identifier(&destination).await?;
            return Ok(ExecutionResult {file:Some(destination.to_string_lossy().into_owned()),sha256:Some(digest),exit_code:Some(0),message:"Arquivo existente reutilizado após conferir SHA-256. Nenhum novo download. Instalação não verificada; integridade não comprova a identidade do fornecedor. Assinatura Authenticode não verificada.".into(),download_metadata:Some(DownloadMetadata {source_url:public_url(&requested),final_url:String::new(),asset_name:spec.filename,version:spec.version,release_url:spec.release_url,sha256_verified:true,origin:spec.origin,reused_existing:true}),..Default::default()});
        }
        bail!(
            "O arquivo já existe: {}. Renomeie ou remova-o antes de baixar novamente.",
            destination.display()
        );
    }
    let temp = folder.join(format!("{}.partial", Uuid::new_v4()));
    let outcome=async {
        (ctx.progress)(Progress {message:"Conectando ao servidor HTTPS".into(),..Default::default()});
        let response_future=client.get(requested.clone()).send();tokio::pin!(response_future);
        let response=loop {if ctx.cancel.load(Ordering::Relaxed){bail!("Download cancelado pelo usuário");}tokio::select! {response=&mut response_future=>break response.map_err(|e|e.without_url())?,_=tokio::time::sleep(Duration::from_millis(200))=>{}}};
        if !response.status().is_success() {bail!("Servidor de download retornou HTTP {}",response.status());}
        let final_url=validate_https(response.url().as_str())?;
        let total=response.content_length().or(spec.size);
        if total.is_some_and(|s|s>MAX_DOWNLOAD_SIZE) {bail!("Download excede o limite de 4 GB");}
        let mut file=tokio::fs::OpenOptions::new().write(true).create_new(true).open(&temp).await?;
        let mut stream=response.bytes_stream();let mut bytes=0_u64;let mut hash=Sha256::new();let mut last=Instant::now()-Duration::from_secs(1);
        loop {
            if ctx.cancel.load(Ordering::Relaxed) {bail!("Download cancelado pelo usuário");}
            let next=tokio::select! {n=stream.next()=>n,_=tokio::time::sleep(Duration::from_millis(200))=>continue};
            let Some(chunk)=next else {break};let chunk=chunk.map_err(|e|e.without_url())?;
            bytes+=chunk.len() as u64;if bytes>MAX_DOWNLOAD_SIZE {bail!("Download excedeu 4 GB");}
            file.write_all(&chunk).await?;hash.update(&chunk);
            if last.elapsed()>Duration::from_millis(120) {(ctx.progress)(Progress {message:format!("Baixando {}",spec.filename),downloaded:bytes,total});last=Instant::now();}
        }
        file.sync_all().await?;drop(file);
        verify_size(bytes,spec.size)?;verify_size(bytes,total)?;
        let digest=hex::encode(hash.finalize());let verified=verify_hash(&digest,spec.sha256.as_deref())?;
        // Preserve Internet origin and never publish a partial/unverified file.
        write_zone_identifier(&temp).await?;
        crate::infrastructure::storage::publish_new_file(&temp,&destination).context("Não foi possível publicar o download sem sobrescrever o destino")?;
        (ctx.progress)(Progress {message:"Download concluído; instalação não verificada".into(),downloaded:bytes,total:Some(bytes)});
        Ok(ExecutionResult {file:Some(destination.to_string_lossy().into_owned()),sha256:Some(digest),exit_code:Some(0),message:format!("Download concluído. {} Instalação não verificada. A origem é uma configuração do usuário; hash não comprova a identidade do fornecedor. Assinatura Authenticode não verificada.",if verified {"SHA-256 conferido com o hash fornecido."}else{"SHA-256 calculado, sem hash publicado/configurado para comparação."}),download_metadata:Some(DownloadMetadata {source_url:public_url(&requested),final_url:public_url(&final_url),asset_name:spec.filename,version:spec.version,release_url:spec.release_url,sha256_verified:verified,origin:spec.origin,reused_existing:false}),..Default::default()})
    }.await;
    if outcome.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    outcome
}
async fn write_zone_identifier(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        tokio::fs::write(
            format!("{}:Zone.Identifier", path.display()),
            b"[ZoneTransfer]\r\nZoneId=3\r\n",
        )
        .await
        .context("Não foi possível marcar o arquivo como download da Internet")?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}
