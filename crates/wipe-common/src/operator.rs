use serde::{Deserialize, Serialize};

/// Operator identity captured in the Certificate of Sanitization.
/// NIST 800-88 Rev. 2 explicitly adds the operator email as a required field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRef {
    pub id: String,
    pub display_name: String,
    pub email: String,
}
