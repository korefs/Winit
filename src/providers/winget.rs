use super::*;
use crate::infrastructure::process;
use anyhow::{Context, Result, bail};
use unicode_width::UnicodeWidthChar;

pub struct WingetProvider;
#[derive(Debug, Clone, Copy)]
pub enum TableKind {
    Search,
    Installed,
    Updates,
}

pub fn common_args(accept: bool) -> Vec<String> {
    let mut args = vec!["--disable-interactivity".into()];
    if accept {
        args.push("--accept-source-agreements".into());
    }
    args
}
pub fn mutation_args(action: Action, id: &str, silent: bool, accept: bool) -> Result<Vec<String>> {
    validate_winget_id(id)?;
    let verb = match action {
        Action::Install => "install",
        Action::Update => "upgrade",
        Action::Uninstall => "uninstall",
        _ => bail!("Operação não suportada pelo Winget"),
    };
    let mut args: Vec<String> = [verb, "--id", id, "--exact", "--source", "winget"]
        .map(String::from)
        .to_vec();
    args.extend(common_args(accept));
    if action != Action::Uninstall && accept {
        args.push("--accept-package-agreements".into());
    }
    if action == Action::Install {
        args.push("--no-upgrade".into());
    }
    if silent {
        args.push("--silent".into());
    }
    Ok(args)
}
impl WingetProvider {
    fn executable() -> Result<PathBuf> {
        let root = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA indisponível")?;
        let exe = PathBuf::from(root)
            .join("Microsoft")
            .join("WindowsApps")
            .join("winget.exe");
        if !exe.exists() {
            bail!(
                "Alias oficial winget.exe ausente. Instale o Instalador de Aplicativo e habilite seu alias em Configurações > Aplicativos > Aliases de execução."
            );
        }
        Ok(exe)
    }
    pub async fn diagnose(&self) -> Result<String> {
        let r = process::run(Self::executable()?, &["--version".into()], true).await.context("Winget não encontrado. Instale ou atualize o Instalador de Aplicativo pela Microsoft Store; depois entre novamente no Windows.")?;
        check_result(&r, false)?;
        Ok(r.stdout.trim().to_owned())
    }
    pub async fn details(&self, id: &str, accept: bool) -> Result<String> {
        validate_winget_id(id)?;
        let mut args = vec![
            "show".into(),
            "--id".into(),
            id.into(),
            "--exact".into(),
            "--source".into(),
            "winget".into(),
        ];
        args.extend(common_args(accept));
        let r = process::run(Self::executable()?, &args, true).await?;
        check_result(&r, false)?;
        Ok(clean_output(&r.stdout))
    }
    async fn table(
        &self,
        mut args: Vec<String>,
        kind: TableKind,
        accept: bool,
    ) -> Result<Vec<Package>> {
        args.extend(["--source".into(), "winget".into()]);
        args.extend(common_args(accept));
        let r = process::run(Self::executable()?, &args, true).await?;
        if is_empty_result(r.exit_code) {
            return Ok(vec![]);
        }
        check_result(&r, false)?;
        parse_table(&r.stdout, kind)
    }
}
#[async_trait]
impl SoftwareProvider for WingetProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Winget
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            search: true,
            installed: true,
            updates: true,
            actions: vec![Action::Install, Action::Update, Action::Uninstall],
        }
    }
    async fn search(&self, query: &str, accept: bool) -> Result<Vec<Package>> {
        let query = query.trim();
        if query.len() > 160 || query.chars().any(char::is_control) {
            bail!("Busca inválida (máximo 160 caracteres, sem controles)");
        }
        if query.len() < 2 {
            return Ok(vec![]);
        }
        self.table(
            vec![
                "search".into(),
                "--query".into(),
                query.into(),
                "--count".into(),
                "100".into(),
            ],
            TableKind::Search,
            accept,
        )
        .await
    }
    async fn installed(&self, accept: bool) -> Result<Vec<Package>> {
        self.table(vec!["list".into()], TableKind::Installed, accept)
            .await
    }
    async fn updates(&self, accept: bool) -> Result<Vec<Package>> {
        self.table(
            vec!["list".into(), "--upgrade-available".into()],
            TableKind::Updates,
            accept,
        )
        .await
    }
    async fn execute(
        &self,
        request: &OperationRequest,
        context: ExecuteContext,
    ) -> Result<ExecutionResult> {
        let args = mutation_args(
            request.action,
            &request.package.id,
            context.silent,
            context.accept_agreements,
        )?;
        (context.progress)(Progress { message:"Winget em execução. O instalador pode solicitar elevação pelo Windows. Aguarde; não há porcentagem confiável.".into(),..Default::default() });
        let mut r = process::run(Self::executable()?, &args, false).await?;
        r.message = if r.exit_code == Some(0) {
            "Operação concluída pelo Winget".into()
        } else {
            format!(
                "{}\n{}",
                exit_description(r.exit_code),
                clean_output(&r.stdout)
                    .chars()
                    .rev()
                    .take(1800)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            )
        };
        Ok(r)
    }
}
pub fn is_empty_result(code: Option<i32>) -> bool {
    matches!(code.map(|c| c as u32), Some(0x8A150014 | 0x8A15002B))
}
pub fn exit_description(code: Option<i32>) -> String {
    let hint = match code.map(|c| c as u32) {
        Some(0x8A150014) => "Nenhum pacote corresponde ao filtro.",
        Some(0x8A15002B) => "Nenhuma atualização aplicável. A versão pode já estar instalada.",
        Some(0x8A15000F) => "A origem de pacotes está indisponível.",
        Some(0x800704C7 | 0x8A15002C) => {
            "A operação foi cancelada ou a elevação não foi concedida."
        }
        _ => {
            "Verifique os detalhes, a rede, os termos do pacote e eventuais solicitações do instalador."
        }
    };
    format!(
        "Winget retornou {}. {hint}",
        code.map(|c| format!("0x{:08X}", c as u32))
            .unwrap_or_else(|| "sem código".into())
    )
}
fn check_result(r: &ExecutionResult, empty_ok: bool) -> Result<()> {
    if r.exit_code != Some(0) && !(empty_ok && is_empty_result(r.exit_code)) {
        bail!(
            "{}\n{}\n{}",
            exit_description(r.exit_code),
            clean_output(&r.stdout),
            r.stderr.trim()
        );
    }
    Ok(())
}

pub fn clean_output(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for b in chars.by_ref() {
                    if ('@'..='~').contains(&b) {
                        break;
                    }
                }
            }
        } else if c == '\r' {
            out.push('\n');
        } else if !c.is_control() || c == '\n' || c == '\t' {
            out.push(c);
        }
    }
    out
}

// Derive column offsets from the header's padding, never translated column names.
// Slice by terminal display columns (not UTF-8 bytes), preserving multiword versions.
pub fn parse_table(raw: &str, kind: TableKind) -> Result<Vec<Package>> {
    let clean = clean_output(raw);
    let lines: Vec<_> = clean.lines().filter(|l| !l.trim().is_empty()).collect();
    let separator = lines
        .iter()
        .position(|l| l.trim().len() >= 10 && l.trim().chars().all(|c| c == '-'));
    let Some(sep) = separator else {
        bail!(
            "Formato de saída Winget não reconhecido. Atualize o Instalador de Aplicativo. Nenhuma operação foi inferida.\n{}",
            clean.chars().take(900).collect::<String>()
        );
    };
    if sep == 0 {
        bail!("Cabeçalho Winget ausente");
    }
    let header = lines[sep - 1];
    let mut starts = vec![0];
    let mut spaces = 0;
    let mut column = 0;
    for c in header.chars() {
        if c == ' ' {
            spaces += 1;
        } else {
            if spaces >= 2 {
                starts.push(column);
            }
            spaces = 0;
        }
        column += c.width().unwrap_or(0);
    }
    if starts.len() < 3 || starts.len() > 6 {
        bail!("Colunas Winget incompatíveis ({})", starts.len());
    }
    let mut packages = vec![];
    let mut seen = std::collections::HashSet::new();
    for line in &lines[sep + 1..] {
        let mut cells = vec![String::new(); starts.len()];
        let mut col = 0;
        for c in line.chars() {
            let idx = starts.partition_point(|s| *s <= col).saturating_sub(1);
            cells[idx].push(c);
            col += c.width().unwrap_or(0);
        }
        let cells: Vec<_> = cells.iter().map(|s| s.trim()).collect();
        if cells[1].is_empty() || cells[2].is_empty() {
            continue;
        } // localized summary/footer
        let id = cells[1];
        let manageable = validate_winget_id(id).is_ok();
        if !manageable && !id.contains('…') {
            continue;
        }
        let reference = PackageRef {
            provider: ProviderId::Winget,
            id: id.into(),
        };
        if !seen.insert(reference.key()) {
            continue;
        }
        packages.push(Package {
            reference,
            name: cells[0].into(),
            version: cells[2].into(),
            available: if matches!(kind, TableKind::Installed | TableKind::Updates)
                && cells.len() > 3
            {
                cells[3].into()
            } else {
                String::new()
            },
            source: "winget".into(),
            manageable,
        });
    }
    Ok(packages)
}
