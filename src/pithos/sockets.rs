use super::commands::CommandType;

use bytemuck;

use std::io::{Error, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};

pub fn write_command_to_daemon_socket(payload: &CommandType) -> Result<String, Error> {
    let write_addr: SocketAddr = SocketAddr::from_abstract_name("pandora")?;
    let mut socket = match UnixStream::connect_addr(&write_addr) {
        Ok(socket) => socket,
        Err(e) => {
            return Err(e);
        }
    };

    let serialized = serde_json::to_string(&payload).expect("could not serialize payload command");
    let payload_length: usize = serialized.len();

    socket.write_all(bytemuck::bytes_of::<usize>(&payload_length))?;
    socket.write_all(serialized.as_bytes())?;
    return read_response_from_daemon_socket(&socket);
}

pub fn read_response_from_daemon_socket(mut socket: &UnixStream) -> Result<String, Error> {
    let mut payload_size: usize = 0;
    socket.read_exact(bytemuck::bytes_of_mut(&mut payload_size)).expect("could not read initial payload length");

    let mut buf: Vec<u8> = vec![0; payload_size];
    socket.read_exact(buf.as_mut_slice()).expect("could not read out payload");

    return Ok(serde_json::from_slice(&buf).expect("could not deserialize payload"));
}

pub fn write_response_to_client_socket(response: &str, mut socket: &UnixStream) -> Result<(), Error> {
    let serialized = serde_json::to_string(response).expect("could not serialize response");
    let payload_length: usize = serialized.len();

    socket.write_all(bytemuck::bytes_of::<usize>(&payload_length))?;
    return socket.write_all(serialized.as_bytes());
}

pub fn read_command_from_client_socket(mut socket: &UnixStream) -> CommandType {
    let mut payload_size: usize = 0;
    socket.read_exact(bytemuck::bytes_of_mut(&mut payload_size)).expect("could not read initial payload length - no response?");

    let mut buf: Vec<u8> = vec![0; payload_size];
    socket.read_exact(buf.as_mut_slice()).expect("could not read out payload");

    return serde_json::from_slice(&buf).expect("could not deserialize payload");
}