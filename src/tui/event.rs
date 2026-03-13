use crossterm::event::KeyEvent;

use crate::ws::WsEvent;

pub enum Event {
    Key(KeyEvent),
    Tick,
    Render,
    WsEvent(WsEvent),
}
