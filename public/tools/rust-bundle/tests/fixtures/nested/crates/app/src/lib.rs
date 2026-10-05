use engine::{self, value::Value as Renamed};
use crate::inner::run;
use external_library::anything;

pub struct Application;
impl Application {
    pub fn start(&self) {}
}
pub trait Runner {
    fn run(&self);
}
pub fn main_entry() {}

mod inner {
    use super::Application;
    pub fn run() {}
    pub enum State { On, Off }
}

mod helpers;
