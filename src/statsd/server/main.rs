#![feature(phase)]
#[phase(syntax, link)]
extern crate log;
extern crate std;
extern crate test;
extern crate sync;
extern crate getopts;
extern crate time;
extern crate collections;

extern crate statsd;

use statsd::server::backend::Backend;
use statsd::server::backends::console::Console;
use statsd::server::backends::graphite::Graphite;
use statsd::server::buckets::Buckets;

use std::from_str::FromStr;
use std::io;
use std::io::{Timer, Listener, Acceptor};
use std::io::net::{addrinfo, tcp};
use std::io::net::ip::{Ipv4Addr, SocketAddr};
use std::io::net::udp::UdpSocket;
use std::option::{Some, None};
use std::result::{Ok, Err};
use std::os;
use std::comm;
use std::str;

use sync::{Mutex, Arc};
use getopts::{optopt, optflag, getopts};


static FLUSH_INTERVAL_MS: u64 = 10000;
static MAX_PACKET_SIZE: uint = 256;

static DEFAULT_UDP_PORT: u16 = 8125;
static DEFAULT_TCP_PORT: u16 = 8126;


/// Different kinds of events we accept in the main event loop.
enum Event {
    FlushTimer,
    UdpMessage(~[u8]),
    TcpMessage(~tcp::TcpStream)
}


/// Run in a new task for each management connection made to the server.
fn management_connection_loop(tcp_stream: ~tcp::TcpStream,
                              buckets_arc: Arc<Mutex<Buckets>>) {
    let mut stream = io::BufferedStream::new(*tcp_stream);
    let mut end_conn = false;

    while !end_conn {

        // XXX: this will fail if non-utf8 characters are used
        let _ = stream.read_line().map(|line| {
            let mut buckets = buckets_arc.lock();
            let (resp, should_end) = buckets.do_management_line(line);

            // TODO: Maybe don't throw away write errors?
            let _ = stream.write(resp.as_bytes());
            let _ = stream.write(['\n' as u8]);
            let _ = stream.flush();

            end_conn = should_end;
        });
    }
}


fn flush_timer_loop(chan: comm::Sender<~Event>, int_ms: u64) {
    let mut timer = Timer::new().unwrap();
    let periodic = timer.periodic(int_ms);

    loop {
        periodic.recv();
        chan.send(~FlushTimer);
    }
}


/// Accept incoming TCP connection to the statsd management port.
fn management_server_loop(chan: comm::Sender<~Event>, port: u16) {
    let addr = SocketAddr { ip: Ipv4Addr(0, 0, 0, 0), port: port };
    let listener = tcp::TcpListener::bind(addr).unwrap();
    let mut acceptor = listener.listen();

    for stream in acceptor.incoming() {
        let _ = stream.map(|stream| {
            chan.send(~TcpMessage(~stream));
        });
    }
}


/// Accept incoming UDP data from statsd clients.
fn udp_server_loop(chan: comm::Sender<~Event>, port: u16) {
    let addr = SocketAddr { ip: Ipv4Addr(0, 0, 0, 0), port: port };
    let mut socket = UdpSocket::bind(addr).unwrap();
    let mut buf = [0u8, ..MAX_PACKET_SIZE];

    loop {
        // TODO: Should handle errors here
        let _ = socket.recvfrom(buf).map(|(nread, _)| {
            // Messages this large probably are bad in some way.
            if nread == MAX_PACKET_SIZE {
                println!("Max packet size exceeded.");
            }

            // Use the slice to strip out trailing \0 characters
            let msg = buf.slice_to(nread).to_owned();
            chan.send(~UdpMessage(msg));
        });
    }
}

fn print_usage() {
    println!("Usage: {} [options]", os::args()[0]);
    println!("  -h --help               Show usage information");
    println!("  --graphite host[:port]  Enable the graphite backend. \
Receiver will default to 2003 if not specified.");
    println!("  --console               Enable console output.");
    println!("  --port port             Have the statsd server listen on this \
UDP port. Defaults to {}.", DEFAULT_UDP_PORT);
    println!("  --admin-port port       Have the admin server listen on this \
TCP port. Defaults to {}.", DEFAULT_TCP_PORT);
    println!("  --flush                 Flush interval, in seconds. Defaults \
to {}.", FLUSH_INTERVAL_MS / 1000);
}


fn main() {
    let args = os::args();

    let opts = ~[
        optflag("h", "help", "Show usage information"),
        optopt("", "graphite", "Enable Graphite backend", "host[:port]"),
        optflag("", "console", "Enable Console output"),
        optopt("", "port", "UDP port for statsd to server listen on", "PORT"),
        optopt("", "admin-port", "TCP port to have admin server listen on", "PORT"),
        optopt("", "flush", "Flush interval, in seconds.", "SECONDS")
    ];

    let matches = match getopts(args.tail(), opts) {
        Ok(m) => { m },
        Err(f) => {
            println!("{}", f.to_err_msg());
            return print_usage();
        }
    };

    if matches.opt_present("h") || matches.opt_present("help") {
        return print_usage();
    }

    let mut backends: ~[~Backend] = ~[];

    if matches.opt_present("graphite") {
        // We can safely unwrap here because getopt handles the error condition
        // for us. Probably.
        let arg_str = matches.opt_str("graphite").unwrap();
        let mut iter = arg_str.split(':');

        let host = iter.next().unwrap();
        let port = match iter.next() {
            Some(port) => match FromStr::from_str(port) {
                Some(port) => port,
                None => {
                    println!("Invalid port number: {}", port);
                    return print_usage();
                }
            },
            None => 2003
        };

        let addr = match addrinfo::get_host_addresses(host) {
            Ok(ref addrs) if addrs.len() > 0 => addrs[0],
            _ => {
                println!("Bad host name {}", host);
                return;
            }
        };

        let backend = Graphite::new(SocketAddr{ip: addr, port: port});
        backends.push(box backend as ~Backend);
        println!("Using graphite backend ({}:{}).", host, port);
    }

    if matches.opt_present("console") {
        backends.push(box Console::new() as ~Backend);
        println!("Using console backend.");
    }

    let udp_port = match matches.opt_str("port") {
        Some(port_str) => match FromStr::from_str(port_str) {
            Some(port) => port,
            None => {
                println!("Invalid port number: {}", port_str);
                return print_usage();
            }
        },
        None => DEFAULT_UDP_PORT
    };

    let tcp_port = match matches.opt_str("admin-port") {
        Some(port_str) => match FromStr::from_str(port_str) {
            Some(port) => port,
            None => {
                println!("Invalid port number: {}", port_str);
                return print_usage();
            }
        },
        None => DEFAULT_TCP_PORT
    };

    let flush_interval = match matches.opt_str("flush") {
        Some(str_secs) => match from_str::<u64>(str_secs) {
            Some(secs) => secs * 1000,
            None => {
                println!("Invalid integer: {}", str_secs);
                return print_usage();
            }
        },
        None => FLUSH_INTERVAL_MS
    };

    let (event_send, event_recv) = comm::channel::<~Event>();

    let flush_send = event_send.clone();
    let mgmt_send = event_send.clone();
    let udp_send = event_send.clone();

    spawn(proc() { flush_timer_loop(flush_send, flush_interval) });
    spawn(proc() { management_server_loop(mgmt_send, tcp_port) });
    spawn(proc() { udp_server_loop(udp_send, udp_port) });

    let buckets = Buckets::new();
    let buckets_arc = Arc::new(Mutex::new(buckets));

    // Main event loop.
    loop {
        match *event_recv.recv() {
            // Flush timeout
            FlushTimer => {
                let mut buckets = buckets_arc.lock();

                for ref mut backend in backends.mut_iter() {
                    backend.flush_buckets(&*buckets);
                }

                buckets.flush();
            },

            // Management server
            TcpMessage(s) => {
                // Clone the arc so the new task gets its own copy.
                let buckets_arc = buckets_arc.clone();

                // Spin up a new thread to handle the TCP stream.
                spawn(proc() { management_connection_loop(s, buckets_arc) });
            },

            // UDP message received
            UdpMessage(buf) => {
                let mut buckets = buckets_arc.lock();
                str::from_utf8(buf)
                    .and_then(|string| FromStr::from_str(string))
                    .map(|metric| buckets.add_metric(metric))
                    .or_else(|| { buckets.bad_messages += 1; None });
            }
        }
    }
}
