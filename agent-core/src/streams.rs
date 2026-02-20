use crate::facts::Fact;
use std::sync::mpsc::{self, Receiver, Sender};

pub fn webhook_channel(_buffer: usize) -> (Sender<Fact>, Receiver<Fact>) {
    mpsc::channel()
}
