use super::*;
use crate::infrastructure::process;
use anyhow::{Result, bail};

#[derive(Clone)]
pub struct WindowsTool {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub executable: &'static str,
    pub args: &'static [&'static str],
    pub modifies: bool,
}
pub const TOOLS: &[WindowsTool] = &[
    WindowsTool {
        id: "task-manager",
        name: "Gerenciador de Tarefas",
        category: "Diagnóstico",
        description: "Processos, desempenho e aplicativos de inicialização.",
        executable: "Taskmgr.exe",
        args: &[],
        modifies: false,
    },
    WindowsTool {
        id: "disks",
        name: "Gerenciamento de Disco",
        category: "Administração",
        description: "Gerencie partições e volumes. Alterações podem apagar dados.",
        executable: "mmc.exe",
        args: &["diskmgmt.msc"],
        modifies: true,
    },
    WindowsTool {
        id: "devices",
        name: "Gerenciador de Dispositivos",
        category: "Administração",
        description: "Inspecione hardware e drivers. Modificações afetam dispositivos.",
        executable: "mmc.exe",
        args: &["devmgmt.msc"],
        modifies: true,
    },
    WindowsTool {
        id: "services",
        name: "Serviços do Windows",
        category: "Administração",
        description: "Inspecione serviços. Parar ou desativar serviços afeta o sistema.",
        executable: "mmc.exe",
        args: &["services.msc"],
        modifies: true,
    },
    WindowsTool {
        id: "system",
        name: "Informações do Sistema",
        category: "Diagnóstico",
        description: "Detalhes do hardware, componentes e ambiente do Windows.",
        executable: "msinfo32.exe",
        args: &[],
        modifies: false,
    },
    WindowsTool {
        id: "directx",
        name: "Diagnóstico do DirectX",
        category: "Diagnóstico",
        description: "Verifique vídeo, áudio e os componentes DirectX.",
        executable: "dxdiag.exe",
        args: &[],
        modifies: false,
    },
    WindowsTool {
        id: "startup",
        name: "Aplicativos de inicialização",
        category: "Configurações",
        description: "Escolha quais aplicativos podem iniciar com o Windows.",
        executable: "ms-settings:startupapps",
        args: &[],
        modifies: true,
    },
    WindowsTool {
        id: "cleanup",
        name: "Limpeza de Disco",
        category: "Manutenção",
        description: "Abra a ferramenta oficial para revisar e escolher arquivos a excluir.",
        executable: "cleanmgr.exe",
        args: &[],
        modifies: true,
    },
    WindowsTool {
        id: "storage",
        name: "Armazenamento",
        category: "Configurações",
        description: "Revise o uso do disco e as recomendações de limpeza.",
        executable: "ms-settings:storagesense",
        args: &[],
        modifies: true,
    },
    WindowsTool {
        id: "update",
        name: "Windows Update",
        category: "Configurações",
        description: "Abra a central oficial de atualizações do Windows.",
        executable: "ms-settings:windowsupdate",
        args: &[],
        modifies: true,
    },
];
pub struct WindowsToolsProvider;
impl WindowsToolsProvider {
    pub fn registry_installed() -> Vec<Package> {
        #[cfg(windows)]
        {
            use winreg::{
                RegKey,
                enums::{
                    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
                    KEY_WOW64_64KEY,
                },
            };
            let mut packages = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (hive, label) in [(HKEY_LOCAL_MACHINE, "HKLM"), (HKEY_CURRENT_USER, "HKCU")] {
                for (view, architecture) in [(KEY_WOW64_64KEY, "x64"), (KEY_WOW64_32KEY, "x86")] {
                    let root = RegKey::predef(hive);
                    let Ok(uninstall) = root.open_subkey_with_flags(
                        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                        KEY_READ | view,
                    ) else {
                        continue;
                    };
                    for key in uninstall.enum_keys().flatten() {
                        let Ok(app) = uninstall.open_subkey(&key) else {
                            continue;
                        };
                        if app.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 {
                            continue;
                        }
                        let Ok(name) = app.get_value::<String, _>("DisplayName") else {
                            continue;
                        };
                        if name.trim().is_empty() {
                            continue;
                        }
                        let version = app
                            .get_value::<String, _>("DisplayVersion")
                            .unwrap_or_else(|_| "não informada".into());
                        if !seen.insert(format!(
                            "{}:{}",
                            name.to_lowercase(),
                            version.to_lowercase()
                        )) {
                            continue;
                        }
                        packages.push(Package {
                            reference: PackageRef {
                                provider: ProviderId::Windows,
                                id: format!("{label}/{architecture}/{key}"),
                            },
                            name,
                            version,
                            available: String::new(),
                            source: "Registro do Windows".into(),
                            manageable: false,
                        });
                    }
                }
            }
            packages.sort_by_key(|p| p.name.to_lowercase());
            packages
        }
        #[cfg(not(windows))]
        {
            vec![]
        }
    }
    pub fn launch(id: &str) -> Result<()> {
        let tool = TOOLS
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow::anyhow!("Ferramenta desconhecida"))?;
        if tool.executable.starts_with("ms-settings:") {
            process::open_url(tool.executable)
        } else {
            let system = std::env::var_os("SystemRoot")
                .ok_or_else(|| anyhow::anyhow!("SystemRoot indisponível"))?;
            let system_dir = PathBuf::from(system).join("System32");
            let exe = system_dir.join(tool.executable);
            let mut cmd = std::process::Command::new(exe);
            for arg in tool.args {
                if arg.ends_with(".msc") {
                    cmd.arg(system_dir.join(arg));
                } else {
                    cmd.arg(arg);
                }
            }
            cmd.current_dir(system_dir).spawn()?;
            Ok(())
        }
    }
}
#[async_trait]
impl SoftwareProvider for WindowsToolsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Windows
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            installed: true,
            actions: vec![Action::Launch],
            ..Default::default()
        }
    }
    async fn installed(&self, _: bool) -> Result<Vec<Package>> {
        Ok(Self::registry_installed())
    }
    async fn execute(
        &self,
        request: &OperationRequest,
        _: ExecuteContext,
    ) -> Result<ExecutionResult> {
        if request.action != Action::Launch {
            bail!("Operação Windows não suportada");
        }
        Self::launch(&request.package.id)?;
        Ok(ExecutionResult {
            exit_code: Some(0),
            message:
                "Ferramenta aberta. As alterações são realizadas na interface oficial do Windows."
                    .into(),
            ..Default::default()
        })
    }
}
