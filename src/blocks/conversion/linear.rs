use crate::blocks::conversion::base::ConversionBlock;
use crate::error::MdfError;
use crate::parsing::decoder::DecodedValue;
use meval::{Context, Expr};

/// Attempts to extract a numeric value from a [`DecodedValue`].
/// Returns `Some(f64)` if the input is numeric, or `None` otherwise.
pub fn extract_numeric(value: &DecodedValue) -> Option<f64> {
    match value {
        DecodedValue::Float(n) => Some(*n),
        DecodedValue::UnsignedInteger(n) => Some(*n as f64),
        DecodedValue::SignedInteger(n) => Some(*n as f64),
        _ => None,
    }
}

/// Apply a linear conversion.
pub fn apply_linear(block: &ConversionBlock, value: DecodedValue) -> Result<DecodedValue, MdfError> {
    if let Some(raw) = extract_numeric(&value) {
        if block.cc_val.len() >= 2 {
            let result = block.cc_val[0] + block.cc_val[1] * raw;
            Ok(DecodedValue::Float(result))
        } else {
            Ok(DecodedValue::Float(raw))
        }
    } else {
        Ok(value)
    }
}

/// Apply a rational conversion.
pub fn apply_rational(block: &ConversionBlock, value: DecodedValue) -> Result<DecodedValue, MdfError> {
    if let Some(raw) = extract_numeric(&value) {
        if block.cc_val.len() >= 6 {
            let p1 = block.cc_val[0];
            let p2 = block.cc_val[1];
            let p3 = block.cc_val[2];
            let p4 = block.cc_val[3];
            let p5 = block.cc_val[4];
            let p6 = block.cc_val[5];

            let num = p1 * raw * raw + p2 * raw + p3;
            let den = p4 * raw * raw + p5 * raw + p6;
            // Always divide: a true zero denominator yields IEEE 754 ±inf/NaN,
            // matching asammdf. Guarding with an epsilon falsified results for
            // legitimate tiny denominators (e.g. 1/x at x = 1e-20).
            Ok(DecodedValue::Float(num / den))
        } else {
            Ok(DecodedValue::Float(raw))
        }
    } else {
        Ok(value)
    }
}

/// Apply an algebraic conversion using a stored formula.
///
/// The formula (variable `X`; the MDF 3 spelling `X1` is accepted too) is
/// parsed once per thread and cached, so a channel read evaluates a parsed
/// expression per sample instead of re-parsing the text. A formula that does
/// not parse or does not evaluate leaves the raw value unchanged.
pub fn apply_algebraic(block: &ConversionBlock, value: DecodedValue) -> Result<DecodedValue, MdfError> {
    if let (Some(raw), Some(formula)) = (extract_numeric(&value), block.formula.as_ref()) {
        Ok(DecodedValue::Float(eval_formula(formula, raw).unwrap_or(raw)))
    } else {
        Ok(value)
    }
}

/// Parsed formulas, by formula text (`None`: the formula does not parse).
/// Bounded: cleared when it grows past [`FORMULA_CACHE_LIMIT`] entries.
type FormulaCache = std::collections::HashMap<String, Option<Expr>>;

const FORMULA_CACHE_LIMIT: usize = 1024;

thread_local! {
    static FORMULAS: std::cell::RefCell<FormulaCache> = std::cell::RefCell::new(FormulaCache::new());
    /// meval's built-in constants and functions (`Context::new` copies them
    /// on every call, which is most of the cost of evaluating a formula).
    static BUILTINS: Context<'static> = Context::new();
}

/// `formula` evaluated at `X = x`, or `None` when it does not parse or
/// evaluate.
fn eval_formula(formula: &str, x: f64) -> Option<f64> {
    let eval = |expr: &Expr| {
        BUILTINS.with(|builtins| expr.eval_with_context((("X", x), (("X1", x), builtins))).ok())
    };
    FORMULAS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(parsed) = cache.get(formula) {
            return eval(parsed.as_ref()?);
        }
        if cache.len() >= FORMULA_CACHE_LIMIT {
            cache.clear();
        }
        // asammdf compatibility: files converted from MDF 3 commonly use `X1`
        // as the variable name.
        let parsed = formula.replace("X1", "X").parse::<Expr>().ok();
        let result = parsed.as_ref().and_then(eval);
        cache.insert(formula.to_string(), parsed);
        result
    })
}
