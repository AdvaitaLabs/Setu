use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use setu_types::{Address, ObjectId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SuiVmStoredValue {
    U64(u64),
}

impl SuiVmStoredValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuiVmStoredObject {
    pub object_id: ObjectId,
    pub type_name: String,
    pub owner: Option<Address>,
    pub version: u64,
    pub fields: BTreeMap<String, SuiVmStoredValue>,
}

impl SuiVmStoredObject {
    pub fn new_owned(
        object_id: ObjectId,
        type_name: impl Into<String>,
        owner: Address,
        fields: BTreeMap<String, SuiVmStoredValue>,
    ) -> Self {
        Self {
            object_id,
            type_name: type_name.into(),
            owner: Some(owner),
            version: 1,
            fields,
        }
    }

    pub fn get_u64_field(&self, field_name: &str) -> Option<u64> {
        self.fields
            .get(field_name)
            .and_then(SuiVmStoredValue::as_u64)
    }

    pub fn set_u64_field(&mut self, field_name: &str, value: u64) {
        self.fields
            .insert(field_name.to_string(), SuiVmStoredValue::U64(value));
        self.version += 1;
    }
}
