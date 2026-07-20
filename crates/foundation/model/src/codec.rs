#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CanonicalReadError;

pub(crate) struct CanonicalReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> CanonicalReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], CanonicalReadError> {
        let end = self.cursor.checked_add(length).ok_or(CanonicalReadError)?;
        let value = self.bytes.get(self.cursor..end).ok_or(CanonicalReadError)?;
        self.cursor = end;
        Ok(value)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, CanonicalReadError> {
        self.take(1)?.first().copied().ok_or(CanonicalReadError)
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, CanonicalReadError> {
        let value = self.take(2)?;
        Ok(u16::from_be_bytes([value[0], value[1]]))
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, CanonicalReadError> {
        let value = self.take(4)?;
        Ok(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
    }

    pub(crate) fn read_u64(&mut self) -> Result<u64, CanonicalReadError> {
        let value: [u8; 8] = self.take(8)?.try_into().map_err(|_| CanonicalReadError)?;
        Ok(u64::from_be_bytes(value))
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.cursor == self.bytes.len()
    }
}
