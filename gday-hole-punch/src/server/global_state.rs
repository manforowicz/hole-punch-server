use crate::{Contact, FullContact, RoomId};
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

use super::ServerError;

#[derive(Default)]
struct Client {
    contact: FullContact,
    waiting: Option<oneshot::Sender<(Contact, FullContact)>>,
}

#[derive(Clone, Default)]
pub struct State {
    /// Maps room_id to clients
    rooms: Arc<Mutex<HashMap<RoomId, [Client; 2]>>>,
}


fn generate_room_id() -> RoomId {
    let mut rng = rand::thread_rng();
    let characters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut id = [0; 6];
    for letter in &mut id {
        *letter = *characters.choose(&mut rng).unwrap();
    }

    id
}

impl State {
    pub fn create_room(&mut self) -> RoomId {
        let mut rooms = self.rooms.lock().unwrap();

        let mut room_id = generate_room_id();
        while rooms.contains_key(&room_id) {
            room_id = generate_room_id();
        }

        rooms.insert(room_id, [Client::default(), Client::default()]);
        self.room_timeout(room_id);

        room_id
    }

    pub fn update_client(
        &mut self,
        room_id: RoomId,
        is_creator: bool,
        endpoint: SocketAddr,
        public: bool,
    ) -> Result<(), ServerError> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(&room_id).ok_or(ServerError::NoSuchRoomExists)?;
        let contact = &mut room[usize::from(is_creator)].contact;

        let contact = if public {
            &mut contact.public
        } else {
            &mut contact.private
        };

        match endpoint {
            SocketAddr::V6(addr) => {
                contact.v6 = Some(addr);
            }
            SocketAddr::V4(addr) => {
                contact.v4 = Some(addr);
            }
        };

        Ok(())
    }

    pub fn set_client_done(
        &mut self,
        room_id: RoomId,
        is_creator: bool,
    ) -> Result<oneshot::Receiver<(Contact, FullContact)>, ServerError> {
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.get_mut(&room_id).ok_or(ServerError::NoSuchRoomExists)?;

        let client_i = usize::from(is_creator);
        let peer_i = usize::from(!is_creator);

        let client = &mut room[client_i];

        let (tx, rx) = oneshot::channel();
        client.waiting = Some(tx);

        let peer = &room[peer_i];

        if peer.waiting.is_some() {
            let client_info = room[client_i].contact;
            let peer_info = peer.contact;

            let client = &mut room[client_i];
            client
                .waiting
                .take()
                .unwrap()
                .send((client_info.public, peer_info))
                .unwrap();

            let peer = &mut room[peer_i];
            peer.waiting
                .take()
                .unwrap()
                .send((peer_info.public, client_info))
                .unwrap();
            rooms.remove(&room_id);
        }

        Ok(rx)
    }

    fn room_timeout(&self, room_id: RoomId) {
        let state = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60 * 10)).await;
            let mut rooms = state.rooms.lock().unwrap();
            rooms.remove(&room_id);
        });
    }
}