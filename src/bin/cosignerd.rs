use cosigning_server::{config::Config, cosignerd::CosignerD};
use daemonize_simple::Daemonize;
use revault_net::{message::cosigner::Sign, noise::PublicKey as NoisePubkey};
use std::{env, net::TcpListener, path::PathBuf, process, str::FromStr};

fn parse_args(args: Vec<String>) -> Option<PathBuf> {
    if args.len() == 1 {
        return None;
    }

    if args.len() != 3 {
        eprintln!("Unknown arguments '{:?}'.", args);
        eprintln!("Only '--conf <configuration file path>' is supported.");
        process::exit(1);
    }

    Some(PathBuf::from(args[2].to_owned()))
}

// This creates the log file automagically if it doesn't exist, and logs on stdout
// if None is given
fn setup_logger(
    log_file: Option<&str>,
    log_level: log::LevelFilter,
) -> Result<(), fern::InitError> {
    let dispatcher = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(log_level);

    if let Some(log_file) = log_file {
        dispatcher.chain(fern::log_file(log_file)?).apply()?;
    } else {
        dispatcher.chain(std::io::stdout()).apply()?;
    }

    Ok(())
}

// Wait for connections from managers on the configured interface and process `sign` messages.
fn daemon_main(mut cosignerd: CosignerD) {
    let host = cosignerd.listen;
    let listener = TcpListener::bind(host).unwrap_or_else(|e| {
        log::error!("Error binding on '{}': '{}'", host, e);
        process::exit(1);
    });
    let managers_noise_pubkeys: Vec<NoisePubkey> =
        cosignerd.managers.iter().map(|m| m.noise_key).collect();

    // We expect a single connection once in a while, there is *no need* for complexity here so
    // just treat incoming connections sequentially.
    for stream in listener.incoming() {
        log::trace!("Got a new connection: '{:?}'", stream);
        let stream = match stream {
            Ok(s) => s,
            Err(e) => continue,
        };
        // This does the Noise KK handshake.
        let mut kk_stream = match revault_net::transport::KKTransport::accept(
            &listener,
            &cosignerd.noise_privkey,
            &managers_noise_pubkeys,
        ) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Error during handshake: '{}'", e);
                continue;
            }
        };

        let buf = match kk_stream.read() {
            Ok(buf) => buf,
            Err(e) => {
                log::error!("Error reading from stream '{:?}': '{}'", stream, e);
                continue;
            }
        };
        log::debug!(
            "Got '{}' from '{}'",
            String::from_utf8_lossy(&buf),
            revault_net::sodiumoxide::hex::encode(&kk_stream.remote_static().0)
        );
        let sign_msg: Sign = match serde_json::from_slice(&buf) {
            Ok(msg) => msg,
            // FIXME: This should probably be fatal, they are violating the protocol
            Err(e) => {
                log::error!("Decoding sign message: '{}'", e);
                continue;
            }
        };
        log::trace!("Decoded: {:#?}", sign_msg);

        // TODO: process sign message
    }
}

fn main() {
    let args = env::args().collect();
    let conf_file = parse_args(args);

    let config = Config::from_file(conf_file).unwrap_or_else(|e| {
        eprintln!("Error parsing config: {}", e);
        process::exit(1);
    });
    let log_level = config.log_level;

    // Construct CosignerD (global state)
    let mut cosignerd = CosignerD::from_config(config).unwrap_or_else(|e| {
        eprintln!("Error creating global state: {}", e);
        process::exit(1);
    });

    let log_file = cosignerd.log_file();
    let log_output = Some(log_file.to_str().expect("Valid unicode"));

    setup_logger(log_output, log_level).unwrap_or_else(|e| {
        eprintln!("Error setting up logger: {}", e);
        process::exit(1);
    });

    // run cosignerd as daemon
    let mut daemon = Daemonize::default();
    daemon.pid_file = Some(cosignerd.pid_file());
    daemon.doit().unwrap_or_else(|e| {
        eprintln!("Error daemonizing: {}", e);
        process::exit(1);
    });
    log::info!("Started cosignerd daemon.");

    daemon_main(cosignerd);
}