//! Module for the structs that represent and manage the validator/full-node processes i.e. Zebrad.

use std::{path::PathBuf, process::Child};

use getset::{CopyGetters, Getters};
use portpicker::Port;
use tempfile::TempDir;

use crate::{config, error::LaunchError, launch, logs, network, Process};

/// Loads chain into validator data directory from cache
pub fn load_chain(chain_cache: PathBuf, validator_data_dir: PathBuf) -> std::process::Output {
    let regtest_dir = chain_cache.join("regtest");
    if !regtest_dir.exists() {
        panic!("regtest directory not found!");
    }

    std::process::Command::new("cp")
        .arg("-r")
        .arg(regtest_dir)
        .arg(validator_data_dir)
        .output()
        .unwrap()
}

/// Zcashd configuration
///
/// Use `zcashd_bin` and `zcash_cli_bin` to specify the paths to the binaries.
/// If these binaries are in $PATH, `None` can be specified to run "zcashd" / "zcash-cli".
///
/// Use `fixed_port` to specify a port for Zcashd. Otherwise, a port is picked at random between 15000-25000.
///
/// Use `activation_heights` to specify custom network upgrade activation heights
///
/// Use `miner_address` to specify the target address for the block rewards when blocks are generated.
pub struct ZcashdConfig {
    /// Zcashd binary location
    pub zcashd_bin: Option<PathBuf>,
    /// Zcash-cli binary location
    pub zcash_cli_bin: Option<PathBuf>,
    /// Zcashd RPC port
    pub rpc_port: Option<Port>,
    /// Local network upgrade activation heights
    pub activation_heights: network::ActivationHeights,
    /// Miner address
    pub miner_address: Option<&'static str>,
    /// Data directory
    pub data_dir: TempDir,
}

impl Default for ZcashdConfig {
    fn default() -> Self {
        let data_dir = tempfile::tempdir().unwrap();
        Self {
            zcashd_bin: None,
            zcash_cli_bin: None,
            rpc_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: None,
            data_dir,
        }
    }
}

/// Functionality for validator/full-node processes.
pub trait Validator: Sized {
    /// Config filename
    const CONFIG_FILENAME: &str;

    /// Validator config struct
    type Config;

    /// Launch the process.
    fn launch(config: Self::Config) -> Result<Self, LaunchError>;

    /// Stop the process.
    fn stop(&mut self);

    /// Generate `n` blocks.
    fn generate_blocks(&self, n: u32) -> std::io::Result<std::process::Output>;

    /// Get temporary config directory.
    fn config_dir(&self) -> &TempDir;

    /// Get temporary logs directory.
    fn logs_dir(&self) -> &TempDir;

    /// Get temporary data directory.
    fn data_dir(&self) -> &TempDir;

    /// Returns path to config file.
    fn config_path(&self) -> PathBuf {
        self.config_dir().path().join(Self::CONFIG_FILENAME)
    }

    /// Caches chain
    fn cache_chain(&self, chain_cache: PathBuf) -> std::process::Output {
        if chain_cache.exists() {
            panic!("chain cache already exists!");
        }

        std::process::Command::new("cp")
            .arg("-r")
            .arg(self.data_dir().path().to_path_buf())
            .arg(chain_cache)
            .output()
            .unwrap()
    }

    /// Prints the stdout log.
    fn print_stdout(&self) {
        let stdout_log_path = self.logs_dir().path().join(logs::STDOUT_LOG);
        logs::print_log(stdout_log_path);
    }

    /// Prints the stdout log.
    fn print_stderr(&self) {
        let stdout_log_path = self.logs_dir().path().join(logs::STDERR_LOG);
        logs::print_log(stdout_log_path);
    }
}

/// This struct is used to represent and manage the Zcashd process.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Zcashd {
    /// Child process handle
    handle: Child,
    /// RPC port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    port: Port,
    /// Config directory
    config_dir: TempDir,
    /// Logs directory
    logs_dir: TempDir,
    /// Data directory
    data_dir: TempDir,
    /// Zcash cli binary location
    zcash_cli_bin: Option<PathBuf>,
}

impl Zcashd {
    /// Runs a Zcash-cli command with the given `args`.
    ///
    /// Example usage for generating blocks in Zcashd local net:
    /// ```ignore (incomplete)
    /// self.zcash_cli_command(&["generate", "1"]);
    /// ```
    pub fn zcash_cli_command(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut command = match &self.zcash_cli_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zcash-cli"),
        };

        command.arg(format!("-conf={}", self.config_path().to_str().unwrap()));
        command.args(args).output()
    }
}

impl Validator for Zcashd {
    const CONFIG_FILENAME: &str = config::ZCASHD_FILENAME;

    type Config = ZcashdConfig;

    fn launch(config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();

        let port = network::pick_unused_port(config.rpc_port);
        let config_dir = tempfile::tempdir().unwrap();
        let config_file_path = config::zcashd(
            config_dir.path(),
            port,
            &config.activation_heights,
            config.miner_address,
        )
        .unwrap();

        let mut command = match config.zcashd_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zcashd"),
        };
        command
            .args([
                "--printtoconsole",
                format!(
                    "--conf={}",
                    config_file_path.to_str().expect("should be valid UTF-8")
                )
                .as_str(),
                format!(
                    "--datadir={}",
                    config
                        .data_dir
                        .path()
                        .to_str()
                        .expect("should be valid UTF-8")
                )
                .as_str(),
                "-debug=1",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut handle = command.spawn().unwrap();

        logs::write_logs(&mut handle, &logs_dir);
        launch::wait(
            Process::Zcashd,
            &mut handle,
            &logs_dir,
            None,
            "init message: Done loading",
            "Error:",
        )?;

        let zcashd = Zcashd {
            handle,
            port,
            config_dir,
            logs_dir,
            data_dir: config.data_dir,
            zcash_cli_bin: config.zcash_cli_bin,
        };

        // generate genesis block (or additional block if loading chain from cache)
        zcashd.generate_blocks(1).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));

        Ok(zcashd)
    }

    fn stop(&mut self) {
        match self.zcash_cli_command(&["stop"]) {
            Ok(_) => {
                if let Err(e) = self.handle.wait() {
                    tracing::error!("zcashd cannot be awaited: {e}")
                } else {
                    tracing::info!("zcashd successfully shut down")
                };
            }
            Err(e) => {
                tracing::error!(
                    "Can't stop zcashd from zcash-cli: {e}\n\
                    Sending SIGKILL to zcashd process."
                );
                if let Err(e) = self.handle.kill() {
                    tracing::warn!("zcashd has already terminated: {e}")
                };
            }
        }
    }

    fn generate_blocks(&self, n: u32) -> std::io::Result<std::process::Output> {
        self.zcash_cli_command(&["generate", &n.to_string()])
    }

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }

    fn data_dir(&self) -> &TempDir {
        &self.data_dir
    }
}

impl Default for Zcashd {
    /// Default launch for Zcashd.
    /// Panics on failure.
    fn default() -> Self {
        Zcashd::launch(ZcashdConfig::default()).unwrap()
    }
}

impl Drop for Zcashd {
    fn drop(&mut self) {
        self.stop();
    }
}
