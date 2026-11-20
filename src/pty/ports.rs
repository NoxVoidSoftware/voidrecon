//! Which TCP ports are listening on this machine and which process owns them,
//! so the LLM knows what its own servers and listeners already occupy.
//!
//! Uses `lsof` (macOS, most Linux) or `ss` (Linux) plus `ps` for the process
//! tree. All functions block: call them through `spawn_blocking`.

use std::collections::HashMap;
use std::process::Command;

/// A listening TCP socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningPort {
    pub port: u16,
    /// Address it is bound to, e.g. `*`, `0.0.0.0`, `127.0.0.1`.
    pub address: String,
    pub pid: u32,
    pub process: String,
}

/// Listening TCP ports, sorted by port, one entry per port and process.
pub fn listening_ports() -> Vec<ListeningPort> {
    let mut ports = run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .map(|output| parse_lsof(&output))
        .filter(|ports| !ports.is_empty())
        .or_else(|| run("ss", &["-ltnpH"]).map(|output| parse_ss(&output)))
        .unwrap_or_default();
    ports.sort_by_key(|port| (port.port, port.pid));
    ports.dedup_by(|a, b| a.port == b.port && a.pid == b.pid);
    ports
}

/// Parent pid of every process (`ps`), for mapping a listener to its tab.
pub fn process_parents() -> HashMap<u32, u32> {
    run("ps", &["-A", "-o", "pid=", "-o", "ppid="])
        .map(|output| {
            output
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `pid` is `ancestor` or one of its descendants.
pub fn descends_from(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match parents.get(&current) {
            Some(&parent) if parent != current && parent > 1 => current = parent,
            _ => return false,
        }
    }
    false
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    // lsof exits 1 when nothing matches; its output is still valid.
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Splits `host:port` (IPv6 hosts in brackets) into address and port.
fn split_address(name: &str) -> Option<(String, u16)> {
    let (host, port) = name.rsplit_once(':')?;
    Some((host.trim_matches(['[', ']']).to_string(), port.parse().ok()?))
}

/// `lsof -F pcn` output: `p<pid>`, `c<command>`, then `n<addr:port>` lines.
fn parse_lsof(output: &str) -> Vec<ListeningPort> {
    let mut ports = Vec::new();
    let mut pid = 0;
    let mut process = String::new();
    for line in output.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = value.parse().unwrap_or(0),
            "c" => process = value.to_string(),
            "n" => {
                if let Some((address, port)) = split_address(value) {
                    ports.push(ListeningPort { port, address, pid, process: process.clone() });
                }
            }
            _ => {}
        }
    }
    ports
}

/// `ss -ltnpH` lines: `LISTEN 0 5 0.0.0.0:8000 0.0.0.0:* users:(("python3",pid=12,fd=3))`.
fn parse_ss(output: &str) -> Vec<ListeningPort> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (address, port) = split_address(fields.get(3)?)?;
            let users = line.split_once("users:((\"").map_or("", |(_, rest)| rest);
            let process = users.split('"').next().unwrap_or("").to_string();
            let pid = users
                .split_once("pid=")
                .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|pid| pid.parse().ok())
                .unwrap_or(0);
            Some(ListeningPort { port, address, pid, process })
        })
        .collect()
}

/// Ports a command line names (`8000`, `:8000`, `-p8000`, `--port=8000`).
pub fn ports_in_command(command: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = command
        .split(|c: char| c.is_whitespace() || c == ':' || c == '=' || c == ',')
        .filter_map(|token| {
            // A bare number, or a flag with the number attached (`-p8000`);
            // not a name ending in digits (`python3`).
            let digits = match token.strip_prefix('-') {
                Some(flag) => flag
                    .trim_start_matches('-')
                    .trim_start_matches(|c: char| c.is_ascii_alphabetic()),
                None => token,
            };
            digits.parse::<u16>().ok()
        })
        .filter(|port| *port > 0)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Whether a command starts something that listens on a port (a server,
/// listener or relay), as opposed to connecting to one.
pub fn starts_listener(command: &str) -> bool {
    let lower = command.to_lowercase();
    const MARKERS: [&str; 14] = [
        "http.server",
        "simplehttpserver",
        "nc -l",
        "ncat -l",
        "nc -nl",
        "nc -vl",
        "-lvnp",
        "-lnvp",
        "-nlvp",
        "tcp-listen",
        "php -s",
        "uploadserver",
        "serve",
        "listen",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
        || lower.split_whitespace().any(|word| word.ends_with("server") || word.ends_with("server.py"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_and_ss_output() {
        let lsof = "p812\ncpython3\nn*:8000\np99\ncnc\nn127.0.0.1:4444\nn[::1]:4444\n";
        let ports = parse_lsof(lsof);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0], ListeningPort { port: 8000, address: "*".into(), pid: 812, process: "python3".into() });
        assert_eq!(ports[2].address, "::1");

        let ss = "LISTEN 0 5 0.0.0.0:8000 0.0.0.0:* users:((\"python3\",pid=1234,fd=3))\n";
        let ports = parse_ss(ss);
        assert_eq!(ports, vec![ListeningPort { port: 8000, address: "0.0.0.0".into(), pid: 1234, process: "python3".into() }]);
    }

    #[test]
    fn finds_ports_and_listener_commands() {
        assert_eq!(ports_in_command("python3 -m http.server 8000"), vec![8000]);
        assert_eq!(ports_in_command("nc -lvnp 4444"), vec![4444]);
        assert_eq!(ports_in_command("python3 -m http.server"), Vec::<u16>::new());
        assert_eq!(ports_in_command("php -S 0.0.0.0:8080 --port=9001 -p9002"), vec![8080, 9001, 9002]);
        assert!(starts_listener("python3 -m http.server 8000"));
        assert!(starts_listener("nc -lvnp 4444"));
        assert!(starts_listener("impacket-smbserver share . -smb2support"));
        assert!(!starts_listener("curl http://10.10.10.5:8000/"));
        assert!(!starts_listener("nmap -p 8000 10.10.10.5"));
    }

    #[test]
    fn walks_the_process_tree() {
        let parents = HashMap::from([(30, 20), (20, 10), (10, 1)]);
        assert!(descends_from(30, 10, &parents));
        assert!(descends_from(10, 10, &parents));
        assert!(!descends_from(30, 40, &parents));
    }
}
