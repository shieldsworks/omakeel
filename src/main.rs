use omakeel::{hub, source::Spec, watch};
use std::{env, path::PathBuf, process::ExitCode};
use tokio::signal::unix::{SignalKind, signal};

const USAGE: &str = "\
usage: omakeel run --source SPEC [--source SPEC ...] [--record FILE] [--socket PATH]
       omakeel watch [--socket PATH]
       omakeel --version

sources:
  serial:/dev/ttyUSB0[:BAUD]  a serial or USB NMEA device; BAUD defaults to 4800
  tcp:HOST:PORT               an NMEA stream over TCP, like a phone sharing its GPS
  replay:FILE                 a recording made with --record, in real time

--record FILE saves every line received, with the time it arrived. It never
overwrites an existing file. The socket defaults to
$XDG_RUNTIME_DIR/omakeel/keel.sock.";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omakeel: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some((command, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };
    match command.as_str() {
        "run" => serve(rest),
        "watch" => {
            let socket = Options::parse(rest, &["--socket"])?.socket()?;
            watch::run(&socket).map_err(|e| e.to_string())
        }
        "--version" | "version" => {
            println!("omakeel {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn serve(args: &[String]) -> Result<(), String> {
    let options = Options::parse(args, &["--source", "--record", "--socket"])?;
    if options.sources.is_empty() {
        return Err(format!("run needs at least one --source\n\n{USAGE}"));
    }
    let config = hub::Config {
        socket: options.socket()?,
        sources: options.sources,
        record: options.record,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let mut terminate = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
        tokio::select! {
            result = hub::run(config) => result.map_err(|e| e.to_string()),
            _ = tokio::signal::ctrl_c() => Ok(()),
            _ = terminate.recv() => Ok(()),
        }
    })
}

#[derive(Default)]
struct Options {
    sources: Vec<Spec>,
    record: Option<PathBuf>,
    socket: Option<PathBuf>,
}

impl Options {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Options, String> {
        let mut options = Options::default();
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            if !allowed.contains(&flag.as_str()) {
                return Err(format!("unexpected '{flag}'\n\n{USAGE}"));
            }
            let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--source" => options.sources.push(value.parse()?),
                "--record" => options.record = Some(value.into()),
                _ => options.socket = Some(value.into()),
            }
        }
        Ok(options)
    }

    fn socket(&self) -> Result<PathBuf, String> {
        match &self.socket {
            Some(path) => Ok(path.clone()),
            None => hub::default_socket().map_err(|e| e.to_string()),
        }
    }
}
