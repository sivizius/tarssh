
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use exitcode;

use env_logger;
use env_logger::Env;
use log::{error, info, warn};

use futures::future::{loop_fn, Loop};
use futures::stream::Stream;
use futures::Future;

use tokio::net::TcpListener;
use tokio::timer::Delay;

use structopt;
use structopt::StructOpt;

static NUM_CLIENTS: AtomicUsize = AtomicUsize::new(0);
static BANNER: &str = "bleep bloop\r\n";

#[cfg(feature = "sandbox")]
use rusty_sandbox;

#[derive(Debug, StructOpt)]
#[structopt(name = "tarssh", about = "A SSH tarpit server")]
struct Config {
    /// Listen address to bind to
    #[structopt(short = "l", long = "listen", default_value = "0.0.0.0:2222")]
    listen: SocketAddr,
    /// Best-effort connection limit
    #[structopt(short = "c", long = "max-clients")]
    max_clients: Option<u32>,
    /// Seconds between responses
    #[structopt(short = "d", long = "delay", default_value = "10")]
    delay: u32,
    /// Verbose level (repeat for more verbosity)
    #[structopt(short = "v", long = "verbose", parse(from_occurrences))]
    verbose: u8
}

fn errx<M: AsRef<str>>(code: i32, message: M) {
    error!("{}", message.as_ref());
    std::process::exit(code);
}

fn main() {
    let opt = Config::from_args();

    let log_level = match opt.verbose {
        0 => "none",
        1 => "info",
        _ => "debug",
    };
    let max_clients = opt.max_clients.unwrap_or(u32::max_value()) as usize;
    let delay = u64::from(opt.delay);

    env_logger::from_env(Env::default().default_filter_or(log_level)).init();

    let listener = TcpListener::bind(&opt.listen)
        .map_err(|err| errx(exitcode::OSERR, format!("bind(), error: {}", err)))
        .expect("unreachable");

    info!("listen, addr: {}", opt.listen);

    #[cfg(feature = "sandbox")]
    {
        let sandboxed = rusty_sandbox::Sandbox::new().sandbox_this_process().is_ok();
        info!("sandbox mode, enabled: {}", sandboxed);
    }

    let server = listener
        .incoming()
        .map_err(|err| error!("accept(), error: {}", err))
        .filter_map(|sock| {
            sock.peer_addr()
                .map_err(|err| error!("peer_addr(), error: {}", err))
                .map(|peer| (sock, peer))
                .ok()
        })
        .filter(move |(_sock, peer)| {
            let connected = NUM_CLIENTS.fetch_add(1, Ordering::Relaxed) + 1;

            if connected > max_clients {
                NUM_CLIENTS.fetch_sub(1, Ordering::Relaxed);
                info!("reject, peer: {}, clients: {}", peer, connected);
                false
            } else {
                info!("connect, peer: {}, clients: {}", peer, connected);
                true
            }
        })
        .for_each(move |(sock, peer)| {
            let start = Instant::now();
            let _ = sock
                .set_recv_buffer_size(1)
                .map_err(|err| warn!("set_recv_buffer_size(), error: {}", err));

            let _ = sock
                .set_send_buffer_size(64)
                .map_err(|err| warn!("set_send_buffer_size(), error: {}", err));

            let tarpit = loop_fn(sock, move |sock| {
                Delay::new(Instant::now() + Duration::from_secs(delay))
                    .map_err(|err| {
                        error!("tokio timer, error: {}", err);
                        std::io::Error::new(std::io::ErrorKind::Other, "timer failure")
                    })
                    .and_then(move |_| tokio::io::write_all(sock, BANNER))
                    .and_then(|(sock, _)| tokio::io::flush(sock))
                    .map(Loop::Continue)
                    .or_else(move |err| {
                        let connected = NUM_CLIENTS.fetch_sub(1, Ordering::Relaxed);
                        info!(
                            "disconnect, peer: {}, duration: {:.2?}, error: {}, clients: {}",
                            peer,
                            start.elapsed(),
                            err,
                            connected - 1
                        );
                        Ok(Loop::Break(()))
                    })
            });
            tokio::spawn(tarpit)
        });

    tokio::run(server);
}
