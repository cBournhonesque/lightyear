// use crate::shared::sequence_greater_than;
use crate::shared::sequence_less_than;

pub struct SequenceList<T> {
    list: Vec<(u16, T)>,
}

impl<T> SequenceList<T> {
    pub fn new() -> Self {
        Self { list: Vec::new() }
    }

    pub fn front(&self) -> Option<&(u16, T)> {
        self.list.get(0)
    }

    pub fn pop_front(&mut self) -> (u16, T) {
        self.list.remove(0)
    }

    pub fn contains_scan_from_back(&self, id: &u16) -> bool {
        let mut index = self.list.len();

        loop {
            if index == 0 {
                // made it all the way through
                return false;
            }

            index -= 1;

            let (old_id, _) = self.list.get(index).unwrap();
            if *old_id == *id {
                return true;
            }
            if sequence_less_than(*old_id, *id) {
                return false;
            }
        }
    }

    pub fn get_mut_scan_from_back<'a>(&'a mut self, id: &u16) -> Option<&'a mut T> {
        let mut index = self.list.len();

        loop {
            if index == 0 {
                // made it all the way through
                return None;
            }

            index -= 1;

            {
                let (old_id, _) = self.list.get(index).unwrap();
                if *old_id == *id {
                    break;
                }
                if sequence_less_than(*old_id, *id) {
                    return None;
                }
            }
        }

        let (_, item) = self.list.get_mut(index).unwrap();
        Some(item)
    }

    pub fn insert_scan_from_back(&mut self, id: u16, item: T) {
        let mut index = self.list.len();

        loop {
            if index == 0 {
                // made it all the way through, insert at front and be done
                self.list.insert(index, (id, item));
                return;
            }

            index -= 1;

            let (old_id, _) = self.list.get(index).unwrap();
            if *old_id == id {
                panic!("duplicates are not allowed");
            }
            if sequence_less_than(*old_id, id) {
                self.list.insert(index + 1, (id, item));
                return;
            }
        }
    }

    pub fn remove_scan_from_front(&mut self, id: &u16) -> Option<T> {
        let mut index = 0;
        let mut remove = false;

        loop {
            if index >= self.list.len() {
                return None;
            }

            let (old_id, _) = self.list.get(index).unwrap();
            if *old_id == *id {
                remove = true;
            }

            if remove {
                return Some(self.list.remove(index).1);
            }

            index += 1;
        }
    }
}
