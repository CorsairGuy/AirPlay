//! CLI parsing and optional Unix control socket for the PCM sender.
use anyhow::{Context, Result, anyhow, bail};
use media::tx::Volume;
use tokio::io::AsyncRead;
use tokio::sync::watch;

#[derive(Debug)]
pub struct Options {
    pub targets: Vec<String>,
    pub volume_db: f32,
    pub latency_ms: u32,
    pub local: Option<::media::output::local_process::Options>,
    pub control_socket: Option<std::path::PathBuf>,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            targets: Vec::new(),
            volume_db: -29.7,
            latency_ms: 500,
            local: None,
            control_socket: None,
        };
        let mut local_program = None;
        let mut local_args = Vec::new();
        let mut local_gain = 1;
        let mut local_gate = None;
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow!("{argument}: нужно значение"))?;
            match argument.as_str() {
                "--to" => {
                    sessions::pcm_sender::parse_endpoint(value)?;
                    options.targets.push(value.clone());
                }
                "--volume" => options.volume_db = value.parse().context("--volume: нужны дБ")?,
                "--latency-ms" => {
                    options.latency_ms =
                        value.parse().context("--latency-ms: нужны миллисекунды")?
                }
                "--local-program" => {
                    if local_program.is_some() {
                        bail!("--local-program задан повторно");
                    }
                    local_program = Some(std::path::PathBuf::from(value));
                }
                "--local-arg" => local_args.push(value.clone()),
                "--local-gain" => {
                    local_gain = value.parse().context("--local-gain: нужно целое 1..4")?
                }
                "--local-gate" => local_gate = Some(std::path::PathBuf::from(value)),
                "--control-socket" => options.control_socket = Some(value.into()),
                other => bail!("неизвестный параметр {other}"),
            }
        }
        options.local = match local_program {
            Some(program) => Some(::media::output::local_process::Options {
                program,
                args: local_args,
                gain: local_gain,
                gate: local_gate,
            }),
            None => {
                if !local_args.is_empty() || local_gain != 1 || local_gate.is_some() {
                    bail!("локальные параметры требуют --local-program");
                }
                None
            }
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<()> {
        if self.targets.is_empty() || self.targets.len() > 8 {
            bail!("укажите от 1 до 8 --to HOST[:PORT]");
        }
        Volume::db(self.volume_db)?;
        if !(250..=2_000).contains(&self.latency_ms) {
            bail!("--latency-ms: допустимо 250..2000 мс");
        }
        if let Some(local) = &self.local {
            local.validate()?;
        }
        if let Some(path) = &self.control_socket {
            if !cfg!(unix) || !path.is_absolute() {
                bail!("control socket requires an absolute Unix path")
            }
        }
        Ok(())
    }
}

pub async fn run<R: AsyncRead + Unpin>(
    options: Options,
    input: R,
    stop: watch::Receiver<bool>,
) -> Result<()> {
    let Options {
        targets,
        volume_db,
        latency_ms,
        local,
        control_socket,
    } = options;
    let core = sessions::pcm_sender::Options {
        targets,
        volume_db,
        latency_ms,
        local,
    };
    if let Some(path) = control_socket {
        #[cfg(unix)]
        return crate::send_ipc::run(core, input, stop, path).await;
        #[cfg(not(unix))]
        {
            let _ = path;
            bail!("control socket unsupported");
        }
    }
    sessions::pcm_sender::run(core, input, stop).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn validates_targets_volume_and_latency_before_connecting() {
        assert!(Options::parse(&args(&["--to", "localhost", "--to", "[::1]:7123"])).is_ok());
        for values in [
            vec![],
            vec!["--to"],
            vec!["--to", "x:0"],
            vec!["--to", "x", "--volume", "NaN"],
            vec!["--to", "x", "--latency-ms", "249"],
            vec!["--to", "x", "--latency-ms", "2001"],
            vec!["--to", "x", "--local-arg", "null"],
            vec!["--to", "x", "--local-program", "relative"],
            vec![
                "--to",
                "x",
                "--local-program",
                "/bin/cat",
                "--local-gain",
                "5",
            ],
        ] {
            assert!(Options::parse(&args(&values)).is_err(), "{values:?}");
        }
    }
}
