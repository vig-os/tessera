use super::*;

/// A durable gc root's declared kind and stored version seal (ADR-0067 D4).
///
/// The kind is checked against the object, and the object bytes and seal are verified.
/// Pins can name products or collections; blocks and external versions cannot be pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinTarget {
    /// The version object's declared kind.
    pub kind: ObjectKind,
    /// The stored version's `blake3:<64 lower-case hex>` seal.
    pub address: String,
}

impl Repository {
    fn pin_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join("refs/pins")
            .join(ref_file_name(RefNamespace::Pins, name)?))
    }

    /// Read a pin, verifying its canonical record, object kind and version seal.
    /// Missing pins return `None`; corrupt records and unreadable targets are errors.
    /// Like lineage reads, this takes no lock and observes an atomically published record.
    pub fn read_pin(&self, name: &str) -> Result<Option<PinTarget>> {
        let path = self.pin_path(name)?;
        self.layout()?;
        self.read_typed_ref(&path)
            .map(|record| record.map(|(kind, address)| PinTarget { kind, address }))
    }

    /// Set a durable pin by compare-and-set under the repository's fenced writer.
    ///
    /// `expected` is the current target seal, or `None` to create an absent pin. A
    /// disagreement returns [`Error::RefMoved`] (its `lineage` field contains the pin name).
    /// The target must already be stored, readable and of the declared kind; a disagreement
    /// returns [`Error::ObjectKindMismatch`]. An absent new target is [`Error::UnstoredPinTarget`].
    /// Reading a damaged existing pin remains [`Error::RefKindMismatch`] or [`Error::RefTargetUnreadable`],
    /// so callers can distinguish store damage from a bad new request. Nothing is published on validation failure.
    /// An explicit replacement may change the kind: pins have no lineage identity or log.
    ///
    /// Storing an object separately leaves a gc window: if gc reclaims it before this call
    /// acquires the writer, this call refuses the missing target. Keep a lineage or pin root
    /// until the new pin lands. Pins survive `forget` and override every retention class
    /// (#906): retention determines edges, never which roots count.
    pub fn set_pin(&self, name: &str, expected: Option<&str>, target: &PinTarget) -> Result<()> {
        let path = self.pin_path(name)?;
        let _writer = self.lock()?;
        self.check_pin(name, expected)?;
        tessera_core::stored::parse_seal(target.address.as_bytes(), "the pin target")?;
        if !self.has_object(&target.address) {
            return Err(Error::UnstoredPinTarget {
                name: name.into(),
                address: target.address.clone(),
            });
        }
        let bytes = self.read_raw(&target.address)?;
        let object = ObjectKind::of(&bytes)?;
        if target.kind != object {
            return Err(Error::ObjectKindMismatch {
                path,
                expected: target.kind,
                object,
            });
        }
        read_version(&bytes, &target.address)?;
        let tmp = self
            .root
            .join(TMP)
            .join(format!("pin-{}", ref_file_name(RefNamespace::Pins, name)?));
        write_durable_via(
            &tmp,
            &path,
            format!("{} {}\n", kind_name(target.kind), target.address).as_bytes(),
        )
    }

    /// Remove a pin durably by compare-and-set under the fenced writer.
    /// Returns whether the pin existed; `None` only matches an absent pin. A stale expected
    /// seal returns [`Error::RefMoved`]. Removal leaves objects in place; gc reclaims them
    /// only if no other root reaches them. Lineages and their logs are independent.
    /// Corrupt pins must be restored before CAS can inspect their target.
    pub fn remove_pin(&self, name: &str, expected: Option<&str>) -> Result<bool> {
        let path = self.pin_path(name)?;
        let _writer = self.lock()?;
        self.check_pin(name, expected)?;
        match remove_file(&path) {
            Ok(()) => {
                sync_dir(&self.root.join("refs/pins"))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn check_pin(&self, name: &str, expected: Option<&str>) -> Result<()> {
        let found = self.read_pin(name)?.map(|pin| pin.address);
        if found.as_deref() != expected {
            return Err(Error::RefMoved {
                lineage: name.into(),
                expected: expected.map(str::to_string),
                found,
            });
        }
        Ok(())
    }

    /// List pins by decoded name, in name order, verifying every record and target.
    /// Invalid filenames, non-regular entries, corrupt records and kind mismatches fail
    /// the listing. Like lineage reads this is not a snapshot of concurrent writes.
    pub fn pins(&self) -> Result<BTreeMap<String, PinTarget>> {
        self.layout()?;
        let mut pins = BTreeMap::new();
        for entry in std::fs::read_dir(self.root.join("refs/pins"))? {
            let entry = entry?;
            let path = entry.path();
            let invalid = || Error::CorruptRef {
                path: path.clone(),
                reason: "noncanonical encoded pin name".into(),
            };
            let encoded = entry.file_name().into_string().map_err(|_| invalid())?;
            let mut bytes = Vec::new();
            let mut input = encoded.as_bytes().iter().copied();
            while let Some(b) = input.next() {
                if b == b'%' {
                    let hex = [
                        input.next().ok_or_else(invalid)?,
                        input.next().ok_or_else(invalid)?,
                    ];
                    let hex = std::str::from_utf8(&hex).map_err(|_| invalid())?;
                    bytes.push(u8::from_str_radix(hex, 16).map_err(|_| invalid())?);
                } else {
                    bytes.push(b);
                }
            }
            let name = String::from_utf8(bytes).map_err(|_| invalid())?;
            if ref_file_name(RefNamespace::Pins, &name).map_err(|_| invalid())? != encoded {
                return Err(invalid());
            }
            if let Some(pin) = self.read_pin(&name)? {
                pins.insert(name, pin);
            }
        }
        Ok(pins)
    }
}
