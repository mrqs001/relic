use crate::error::{Error, Result};
use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};
#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub data: PathBuf,
    pub address: SocketAddr,
    pub origins: Vec<String>,
    pub api_token: Option<String>,
    pub source_values: BTreeMap<String, String>,
    pub session_dir: PathBuf,
    /// The app was started with an explicit private configuration file
    /// (`--env`). Only then does real background collection that needs no
    /// account (MSCIO) run. Source values alone do not count: tests insert
    /// synthetic ones, and `load` also reads process environment variables.
    pub explicit_env_file: bool,
}
impl Config {
    pub fn load() -> Result<Self> {
        Self::from_args(std::env::args().skip(1))
    }
    /// `load` with explicit command-line arguments (without the program name).
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut root = std::env::current_dir()?;
        let mut env_file = None;
        let mut data = None;
        let mut port = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--root" => {
                    root = PathBuf::from(
                        args.next()
                            .ok_or_else(|| Error::invalid("--root needs a path"))?,
                    )
                }
                "--env" => {
                    env_file =
                        Some(PathBuf::from(args.next().ok_or_else(|| {
                            Error::invalid("--env needs an explicit file")
                        })?))
                }
                "--data-dir" => {
                    data = Some(
                        args.next()
                            .ok_or_else(|| Error::invalid("--data-dir needs a path"))?,
                    )
                }
                "--port" => {
                    port = Some(
                        args.next()
                            .ok_or_else(|| Error::invalid("--port needs a number"))?,
                    )
                }
                _ => {
                    return Err(Error::invalid(
                        "Unknown argument; supported: --root --env --data-dir --port",
                    ));
                }
            }
        }
        root = root.canonicalize()?;
        let mut vars: HashMap<String, String> = std::env::vars()
            .filter(|(k, _)| {
                k.starts_with("RELIC_")
                    || k.starts_with("X_")
                    || k.starts_with("TELEGRAM_")
                    || k.starts_with("OPENROUTER_")
                    || k.starts_with("AI_")
                    || k.starts_with("FR24_")
                    || k.starts_with("KEELWATCH_")
                    || k.starts_with("MARINETRAFFIC_")
            })
            .collect();
        let mut base = root.clone();
        let explicit_env_file = env_file.is_some();
        if let Some(path) = env_file {
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            base = path.parent().unwrap_or(&root).to_path_buf();
            for item in dotenvy::from_path_iter(&path).map_err(|_| {
                Error::invalid("Cannot read the explicitly selected configuration file")
            })? {
                let (k, v) =
                    item.map_err(|_| Error::invalid("Invalid explicit configuration file"))?;
                vars.entry(k).or_insert(v);
            }
        }
        let host: IpAddr = vars
            .get("RELIC_HOST")
            .map(String::as_str)
            .unwrap_or("127.0.0.1")
            .parse()
            .map_err(|_| Error::invalid("RELIC_HOST must be a loopback IP address"))?;
        if !host.is_loopback() {
            return Err(Error::invalid(
                "This local release only supports loopback binding; external access requires an explicit access boundary",
            ));
        }
        let port = port
            .or_else(|| vars.get("RELIC_PORT").cloned())
            .unwrap_or_else(|| "8790".into())
            .parse()
            .map_err(|_| Error::invalid("RELIC_PORT is invalid"))?;
        let data = PathBuf::from(
            data.or_else(|| vars.get("RELIC_DATA_DIR").cloned())
                .unwrap_or_else(|| "data".into()),
        );
        let data = if data.is_absolute() {
            data
        } else {
            base.join(data)
        };
        let mut config = Self::local(root, data, SocketAddr::new(host, port));
        config.explicit_env_file = explicit_env_file;
        // Owner-directed local mode: legacy tokens never enable a workspace
        // password. Loopback binding and Host/Origin checks remain mandatory.
        vars.remove("RELIC_API_TOKEN");
        let sessions = PathBuf::from(
            vars.get("RELIC_SESSION_DIR")
                .map(String::as_str)
                .unwrap_or("private/sessions"),
        );
        config.session_dir = if sessions.is_absolute() {
            sessions
        } else {
            base.join(sessions)
        };
        config.source_values = vars
            .into_iter()
            .filter(|(k, _)| {
                k.starts_with("X_")
                    || k.starts_with("TELEGRAM_")
                    || k.starts_with("OPENROUTER_")
                    || k.starts_with("AI_")
                    || k.starts_with("FR24_")
                    || k.starts_with("KEELWATCH_")
                    || k.starts_with("MARINETRAFFIC_")
            })
            .collect();
        Ok(config)
    }
    pub fn local(root: PathBuf, data: PathBuf, address: SocketAddr) -> Self {
        let session_dir = root.join("private/sessions");
        Self {
            root,
            data,
            address,
            api_token: None,
            source_values: BTreeMap::new(),
            session_dir,
            explicit_env_file: false,
            origins: vec![
                format!("http://{address}"),
                format!("http://localhost:{}", address.port()),
                format!("http://127.0.0.1:{}", address.port()),
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
            ],
        }
    }
    pub fn db_path(&self) -> PathBuf {
        self.data.join("relic.sqlite3")
    }
    pub fn objects(&self) -> PathBuf {
        self.data.join("objects/sha256")
    }
    pub fn object_path(&self, h: &str) -> PathBuf {
        self.objects().join(&h[..2]).join(h)
    }
    pub fn under_root(&self, p: impl AsRef<Path>) -> PathBuf {
        self.root.join(p)
    }
}
