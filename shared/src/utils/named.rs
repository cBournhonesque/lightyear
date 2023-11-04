pub trait TypeNamed {
    fn name() -> String;
}

pub trait Named {
    fn name(&self) -> String;
}
