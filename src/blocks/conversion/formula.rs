use crate::blocks::conversion::base::ConversionBlock;
use crate::blocks::conversion::types::ConversionType;
use crate::blocks::common::read_string_block;
use crate::error::MdfError;

impl ConversionBlock {
    /// Resolves and stores the algebraic formula string (if any) into `self.formula`.
    pub fn resolve_formula(&mut self, file_data: &[u8]) -> Result<(), MdfError> {
        if self.cc_type != ConversionType::Algebraic || self.cc_ref.is_empty() {
            return Ok(());
        }

        let addr = self.cc_ref[0];
        if let Some(formula) = read_string_block(file_data, addr)? {
            self.formula = Some(formula);
        }

        Ok(())
    }
}
