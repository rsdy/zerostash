use crate::backends::{Backend, BackendError};
use crate::chunks::ChunkPointer;

use crate::compress;
use crate::crypto::*;
use crate::BLOCK_SIZE;

use itertools::Itertools;
use thiserror::Error;

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::string::ToString;
use std::sync::{Arc, Mutex};

#[derive(Error, Debug)]
pub enum ObjectError {
    #[error("IO error")]
    Io {
        #[from]
        source: io::Error,
    },
    #[error("Backend error")]
    Backend {
        #[from]
        source: BackendError,
    },
}

pub type Result<T> = std::result::Result<T, ObjectError>;

pub trait ObjectStore: Clone + Send {
    fn store_chunk(&mut self, hash: &CryptoDigest, data: &[u8]) -> Result<Arc<ChunkPointer>>;
    fn flush(&mut self) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectId(CryptoDigest);
pub type WriteObject = Object<BlockBuffer>;
pub type ReadObject = Object<ReadBuffer>;

impl ObjectId {
    #[inline(always)]
    pub fn new(random: &impl Random) -> ObjectId {
        let mut id = ObjectId::default();
        id.reset(random);
        id
    }

    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> ObjectId {
        let mut id = ObjectId::default();
        id.0.copy_from_slice(bytes.as_ref());

        id
    }

    #[inline(always)]
    pub fn reset(&mut self, random: &impl Random) {
        random.fill(&mut self.0);
    }
}

impl AsRef<[u8]> for ObjectId {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl ToString for ObjectId {
    #[inline(always)]
    fn to_string(&self) -> String {
        format!("{:02x}", self.0.as_ref().iter().format(""))
    }
}

#[derive(Clone)]
pub struct BlockBuffer(Box<[u8]>);
pub struct ReadBuffer(ReadBufferInner);
pub type ReadBufferInner = Box<dyn AsRef<[u8]> + Send + Sync + 'static>;

impl<WO> From<WO> for ReadObject
where
    WO: AsRef<WriteObject>,
{
    fn from(rwr: WO) -> ReadObject {
        let rw = rwr.as_ref();

        Object::with_id(
            rw.id,
            ReadBuffer(Box::new(rw.buffer.clone()) as ReadBufferInner),
        )
    }
}

impl ReadBuffer {
    pub fn new(buf: impl AsRef<[u8]> + Send + Sync + 'static) -> ReadBuffer {
        ReadBuffer(Box::new(buf) as ReadBufferInner)
    }
}

impl AsRef<[u8]> for ReadBuffer {
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_ref()
    }
}

impl Default for BlockBuffer {
    #[inline]
    fn default() -> BlockBuffer {
        BlockBuffer(vec![0; BLOCK_SIZE].into_boxed_slice())
    }
}

impl AsMut<[u8]> for BlockBuffer {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut [u8] {
        self.0.as_mut()
    }
}

impl AsRef<[u8]> for BlockBuffer {
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

pub struct Object<T> {
    pub id: ObjectId,
    pub buffer: T,
    capacity: usize,
    cursor: usize,
}

impl<T> Object<T> {
    pub fn new(buffer: T) -> Self {
        Object {
            id: ObjectId::default(),
            cursor: 0,
            capacity: BLOCK_SIZE,
            buffer,
        }
    }
}

impl<T> Object<T> {
    #[inline(always)]
    pub fn set_id(&mut self, id: ObjectId) {
        self.id = id;
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline(always)]
    pub fn position(&self) -> usize {
        self.cursor
    }

    #[inline(always)]
    pub fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    pub fn reserve_tag(&mut self) {
        self.capacity = BLOCK_SIZE - size_of::<Tag>();
    }
}

impl<T> Object<T>
where
    T: AsRef<[u8]>,
{
    pub fn with_id(id: ObjectId, buffer: T) -> Object<T> {
        let mut object = Object {
            id: ObjectId::default(),
            cursor: 0,
            capacity: buffer.as_ref().len(),
            buffer,
        };
        object.set_id(id);
        object
    }
}

impl<T> Object<T>
where
    T: AsMut<[u8]>,
{
    #[inline]
    pub fn clear(&mut self) {
        for i in self.buffer.as_mut().iter_mut() {
            *i = 0;
        }
    }

    #[inline(always)]
    pub fn write_tag(&mut self, buf: &[u8]) {
        self.buffer.as_mut()[self.capacity..].copy_from_slice(buf);
    }

    #[inline(always)]
    pub fn write_head(&mut self, buf: &[u8]) {
        self.buffer.as_mut()[..buf.len()].copy_from_slice(buf);
    }

    #[inline(always)]
    pub fn finalize(&mut self, random: &impl Random) {
        random.fill(&mut self.buffer.as_mut()[self.cursor..])
    }
}

impl<T> Write for Object<T>
where
    T: AsMut<[u8]>,
{
    #[inline(always)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ofs = self.cursor;
        let len = buf.len();

        self.buffer.as_mut()[ofs..(ofs + len)].copy_from_slice(buf);
        self.cursor += len;

        Ok(len)
    }

    #[inline(always)]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T> Read for Object<T>
where
    T: AsRef<[u8]>,
{
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let end = buf.len() + self.cursor;

        if end > self.buffer.as_ref().len() {
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        } else {
            buf.copy_from_slice(&self.buffer.as_ref()[self.cursor..end]);
            self.cursor = end;
            Ok(buf.len())
        }
    }
}

impl<T> Seek for Object<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        use SeekFrom::*;

        let umax = self.capacity as u64;
        let imax = self.capacity as i64;

        match pos {
            Start(s) => match s {
                s if s > umax => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                s => {
                    self.cursor = s as usize;
                    Ok(self.cursor as u64)
                }
            },
            End(e) => match e {
                e if e < 0 => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                e if e > imax => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                e => {
                    self.cursor = self.capacity - e as usize;
                    Ok(self.cursor as u64)
                }
            },
            Current(c) => {
                let new_pos = self.cursor as i64 + c;

                match new_pos {
                    p if p < 0 => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                    p if p > imax => Err(io::Error::from(io::ErrorKind::InvalidInput)),
                    p => {
                        self.cursor = p as usize;
                        Ok(self.cursor as u64)
                    }
                }
            }
        }
    }
}

impl<T> Clone for Object<T>
where
    T: Clone,
{
    fn clone(&self) -> Object<T> {
        Object {
            id: self.id,
            buffer: self.buffer.clone(),
            capacity: self.capacity,
            cursor: self.cursor,
        }
    }
}

impl<T> Default for Object<T>
where
    T: Default + AsRef<[u8]>,
{
    fn default() -> Object<T> {
        let buffer = T::default();
        Object {
            id: ObjectId::default(),
            cursor: 0,
            capacity: buffer.as_ref().len(),
            buffer,
        }
    }
}

impl<T> AsRef<[u8]> for Object<T>
where
    T: AsRef<[u8]>,
{
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        &self.buffer.as_ref()[..self.capacity]
    }
}

impl<T> AsMut<[u8]> for Object<T>
where
    T: AsMut<[u8]>,
{
    #[inline(always)]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buffer.as_mut()[..self.capacity]
    }
}

impl<T> AsMut<Object<T>> for Object<T> {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut Object<T> {
        self
    }
}

impl<T> AsRef<Object<T>> for Object<T> {
    #[inline(always)]
    fn as_ref(&self) -> &Object<T> {
        self
    }
}

pub struct Storage<C> {
    backend: Arc<dyn Backend>,
    crypto: C,
    object: WriteObject,
    capacity: usize,
}

impl<C> Clone for Storage<C>
where
    C: Random + Clone,
{
    fn clone(&self) -> Storage<C> {
        let mut object = self.object.clone();
        object.id.reset(&self.crypto);

        Storage {
            object,
            backend: self.backend.clone(),
            crypto: self.crypto.clone(),
            capacity: self.capacity,
        }
    }
}

impl<C> Storage<C>
where
    C: CryptoProvider,
{
    pub fn new(backend: Arc<dyn Backend>, crypto: C) -> Storage<C> {
        let mut object = WriteObject::default();
        object.id.reset(&crypto);

        let capacity = object.capacity();
        Storage {
            object,
            backend,
            crypto,
            capacity,
        }
    }
}

impl<C> ObjectStore for Storage<C>
where
    C: CryptoProvider,
{
    fn store_chunk(&mut self, hash: &CryptoDigest, data: &[u8]) -> Result<Arc<ChunkPointer>> {
        let mut compressed = compress::block(&data)?;
        let size = compressed.len();
        let mut offs = self.object.position();
        if offs + size > self.capacity {
            self.flush()?;
            offs = self.object.position();
        }

        let tag = self
            .crypto
            .encrypt_chunk(&self.object, hash, &mut compressed);

        self.object.write_all(&compressed)?;

        Ok(Arc::new(ChunkPointer {
            offs: offs as u32,
            size: size as u32,
            file: self.object.id,
            hash: *hash,
            tag,
        }))
    }

    fn flush(&mut self) -> Result<()> {
        self.object.finalize(&self.crypto);
        self.backend.write_object(&self.object)?;

        self.object.id.reset(&self.crypto);
        self.object.reset_cursor();

        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct NullStorage(pub Arc<Mutex<usize>>);

impl ObjectStore for NullStorage {
    fn store_chunk(&mut self, _hash: &CryptoDigest, data: &[u8]) -> Result<Arc<ChunkPointer>> {
        *self.0.lock().unwrap() += data.len();
        Ok(Arc::default())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
