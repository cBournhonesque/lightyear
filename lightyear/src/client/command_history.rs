use std::collections::VecDeque;

use crate::shared::{sequence_greater_than, Tick};

pub struct CommandHistory<T: Clone> {
    buffer: VecDeque<(Tick, T)>,
}

impl<T: Clone> Default for CommandHistory<T> {
    fn default() -> Self {
        Self {
            buffer: VecDeque::default(),
        }
    }
}

impl<T: Clone> CommandHistory<T> {
    pub fn replays(&mut self, start_tick: &Tick) -> Vec<(Tick, T)> {
        // Remove history of commands until current received tick
        self.remove_to_and_including(*start_tick);

        // Get copies of all remaining stored Commands
        let mut output = Vec::new();

        for (tick, command) in self.buffer.iter() {
            output.push((*tick, command.clone()));
        }

        output
    }

    // this only goes forward
    pub fn insert(&mut self, command_tick: Tick, new_command: T) {
        if let Some((last_most_recent_command_tick, _)) = self.buffer.front() {
            if !sequence_greater_than(command_tick, *last_most_recent_command_tick) {
                panic!("You must always insert a more recent command into the CommandHistory than the one you last inserted.");
            }
        }

        // go ahead and push
        self.buffer.push_front((command_tick, new_command));
    }

    fn remove_to_and_including(&mut self, index: Tick) {
        loop {
            let back_index = match self.buffer.back() {
                Some((index, _)) => *index,
                None => {
                    return;
                }
            };
            if sequence_greater_than(back_index, index) {
                return;
            }
            self.buffer.pop_back();
        }
    }

    pub fn can_insert(&self, tick: &Tick) -> bool {
        if let Some((last_most_recent_command_tick, _)) = self.buffer.front() {
            if !sequence_greater_than(*tick, *last_most_recent_command_tick) {
                return false;
            }
        }
        true
    }
}
