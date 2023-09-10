#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use postcard::{from_bytes, to_stdvec};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::num::TryFromIntError;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Error, Debug)]
pub enum Error {
    #[error("Error with encoding/decoding message: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("Message is longer than max of {} bytes", u16::MAX)]
    MessageTooLong(#[from] TryFromIntError),

    #[error("IO Error: {0}")]
    IO(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub enum ClientMessage {
    /// Request the server to create a room
    CreateRoom,
    /// (password, user is creator of room?, private contact, done sending all info)
    SendContact([u8; 6], bool, SocketAddr, bool),
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub enum ServerMessage {
    /// Room successfully created
    /// (room_password, user_id)
    RoomCreated([u8; 6]),
    /// (full contact info of peer)
    SharePeerContacts(FullContact),
    SyntaxError,
    NoSuchRoomPasswordError,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct Contact {
    pub v6: Option<SocketAddrV6>,
    pub v4: Option<SocketAddrV4>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct FullContact {
    pub private: Contact,
    pub public: Contact,
}

pub async fn deserialize_from<T: AsyncReadExt + Unpin, U: DeserializeOwned>(
    stream: &mut T,
) -> Result<U, Error> {
    let length = stream.read_u16().await? as usize;
    let mut buf = vec![0; length];
    stream.read_exact(&mut buf).await?;
    Ok(from_bytes(&buf)?)
}

pub async fn serialize_into<T: AsyncWriteExt + Unpin, U: Serialize>(
    stream: &mut T,
    msg: &U,
) -> Result<(), Error> {
    let msg = to_stdvec(msg)?;
    let length = u16::try_from(msg.len())?.to_be_bytes();
    Ok(stream.write_all(&[&length[..], &msg[..]].concat()).await?)
}
