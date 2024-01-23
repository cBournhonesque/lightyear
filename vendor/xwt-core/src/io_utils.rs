use crate::io;

pub type ReadErrorFor<T> = <T as io::Read>::Error;

pub type WriteErrorFor<T> = <T as io::Write>::Error;

pub type ReadChunkErrorFor<T, Data> = <T as io::ReadChunk<Data>>::Error;

pub type WriteChunkErrorFor<T, Data> = <T as io::WriteChunk<Data>>::Error;
