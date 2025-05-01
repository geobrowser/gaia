use std::collections::HashMap;

pub struct Storage {
    store: HashMap<String, String>,
}

impl Storage {
    pub fn new() -> Self {
        Storage {
            store: HashMap::new(),
        }
    }
}

pub struct Cache {
    storage: Storage,
}

impl Cache {
    pub fn new(storage: Storage) -> Self {
        Cache { storage }
    }

    pub fn get(&self, key: &String) -> Option<&String> {
        return self.storage.get(key);
    }

    pub fn put(&mut self, key: &String, value: &String) {
        self.storage.put(key, value);
    }
}

pub trait Writable {
    fn put(&mut self, key: &String, value: &String);
}

pub trait Readable {
    fn get(&self, key: &String) -> Option<&String>;
}

impl Writable for Storage {
    fn put(&mut self, key: &String, value: &String) {
        self.store.insert(key.clone(), value.clone());
    }
}

impl Readable for Storage {
    fn get(&self, key: &String) -> Option<&String> {
        return self.store.get(key);
    }
}
