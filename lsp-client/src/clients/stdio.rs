use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::client::Client;

pub fn stdio_client(
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
) -> (Client, [JoinHandle<()>; 3], Arc<AtomicBool>) {
    let (client_tx, client_rx) = channel::<String>();
    let (server_tx, server_rx) = channel();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_input = stop_flag.clone();
    let stop_flag_output = stop_flag.clone();
    let stop_flag_error = stop_flag.clone();

    let server_input_handle = std::thread::spawn(move || {
        while !stop_flag_input.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(msg) = client_rx.recv_timeout(Duration::from_millis(10)) {
                stdin.write_all(msg.as_bytes()).unwrap();
            }
        }
    });

    let server_output_handle = stdout_proxy(BufReader::new(stdout), server_tx, stop_flag_output);

    let mut stderr_lines = BufReader::new(stderr).lines();
    let server_error_handle = std::thread::spawn(move || {
        while !stop_flag_error.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(Ok(line)) = stderr_lines.next() {
                eprintln!("Got err from server: {}", line);
            }
        }
    });

    let client = Client::new(client_tx, server_rx);

    (
        client,
        [
            server_input_handle,
            server_output_handle,
            server_error_handle,
        ],
        stop_flag,
    )
}

fn stdout_proxy(
    mut rx: BufReader<ChildStdout>,
    tx: Sender<String>,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut next_content_length = None;
        let mut next_content_type = None;

        while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
            let mut line = String::new();
            if rx.read_line(&mut line).is_err() {
                break;
            }

            let words = line.split_ascii_whitespace().collect::<Vec<_>>();
            match (
                words.as_slice(),
                &mut next_content_length,
                &mut next_content_type,
            ) {
                (["Content-Length:", content_length], None, None) => {
                    next_content_length = Some(content_length.parse().unwrap())
                }
                (["Content-Type:", content_type], Some(_), None) => {
                    next_content_type = Some(content_type.to_string())
                }
                ([], Some(content_length), _) => {
                    let mut content = Vec::with_capacity(*content_length);
                    let mut bytes_left = *content_length;
                    while bytes_left > 0 {
                        let read_bytes = rx.read_until(b'}', &mut content).unwrap();
                        bytes_left -= read_bytes;
                    }

                    let content = String::from_utf8(content).unwrap();
                    tx.send(content).unwrap();

                    next_content_length = None;
                    next_content_type = None;
                }
                // empty line only for server termination
                ([], None, None) => {
                    println!("Server shutting down...");
                    break;
                }
                unexpected => panic!("Got unexpected stdout: {:?}", unexpected),
            };
        }
    })
}
