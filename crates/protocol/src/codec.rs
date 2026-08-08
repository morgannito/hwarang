//! Lecture et ecriture des champs d'une charge utile.

use crate::DecodeError;

/// Lecteur sequentiel sur une charge utile.
///
/// [`Reader::finish`] impose de consommer exactement les octets annonces :
/// accepter un reliquat laisserait passer des trames dont la taille ne
/// correspond pas a l'opcode, terrain classique de la confusion de type.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (head, tail) = self
            .bytes
            .split_at_checked(N)
            .ok_or(DecodeError::MalformedPayload)?;
        self.bytes = tail;
        head.try_into().map_err(|_| DecodeError::MalformedPayload)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        self.take::<1>().map(|[byte]| byte)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        self.take::<2>().map(u16::from_be_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        self.take::<4>().map(u32::from_be_bytes)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        self.take::<8>().map(u64::from_be_bytes)
    }

    pub(crate) fn i32(&mut self) -> Result<i32, DecodeError> {
        self.take::<4>().map(i32::from_be_bytes)
    }

    /// Echoue s'il reste des octets non lus.
    pub(crate) const fn finish(self) -> Result<(), DecodeError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::MalformedPayload)
        }
    }
}

/// Accumulateur de charge utile.
#[derive(Default)]
pub(crate) struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn u8(mut self, value: u8) -> Self {
        self.bytes.push(value);
        self
    }

    pub(crate) fn u16(mut self, value: u16) -> Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn u32(mut self, value: u32) -> Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn u64(mut self, value: u64) -> Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn i32(mut self, value: i32) -> Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_les_champs_dans_l_ordre_ecrit() {
        let bytes = Writer::default()
            .u64(0x0123_4567_89AB_CDEF)
            .i32(-42)
            .u16(7)
            .into_bytes();

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u64(), Ok(0x0123_4567_89AB_CDEF));
        assert_eq!(reader.i32(), Ok(-42));
        assert_eq!(reader.u16(), Ok(7));
        assert_eq!(reader.finish(), Ok(()));
    }

    #[test]
    fn une_charge_trop_courte_est_rejetee() {
        let bytes = Writer::default().u16(1).into_bytes();
        assert_eq!(
            Reader::new(&bytes).u64(),
            Err(DecodeError::MalformedPayload)
        );
    }

    #[test]
    fn un_reliquat_non_lu_est_rejete() {
        let bytes = Writer::default().u32(1).u32(2).into_bytes();
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u32(), Ok(1));
        assert_eq!(reader.finish(), Err(DecodeError::MalformedPayload));
    }

    #[test]
    fn les_entiers_signes_survivent_a_l_aller_retour() {
        for value in [i32::MIN, -1, 0, 1, i32::MAX] {
            let bytes = Writer::default().i32(value).into_bytes();
            assert_eq!(Reader::new(&bytes).i32(), Ok(value));
        }
    }
}
