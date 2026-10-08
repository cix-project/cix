#![cfg(feature = "server")]
//! Actual emitted Count schema and serde boundary agreement; separately qualified.
use cix_native::service::contracts::Count;
use serde_json::json;

#[test]
fn emitted_counter_schema_preserves_unsigned_decimal_contract() {
    let emitted = serde_json::to_value(schemars::schema_for!(Count)).unwrap();
    assert_eq!(<Count as schemars::JsonSchema>::schema_name(), "UInt64Decimal");
    assert_eq!(emitted["type"], json!("string"));
    assert_eq!(emitted["maxLength"], json!(20));
    assert_eq!(emitted["description"], json!("Unsigned 64-bit decimal integer; maximum 18446744073709551615."));
    assert_eq!(emitted["pattern"], json!("^(?:0|[1-9][0-9]{0,18}|1[0-7][0-9]{18}|18[0-3][0-9]{17}|184[0-3][0-9]{16}|1844[0-5][0-9]{15}|18446[0-6][0-9]{14}|184467[0-3][0-9]{13}|1844674[0-3][0-9]{12}|184467440[0-6][0-9]{10}|1844674407[0-2][0-9]{9}|18446744073[0-6][0-9]{8}|1844674407370[0-8][0-9]{6}|18446744073709[0-4][0-9]{5}|184467440737095[0-4][0-9]{4}|18446744073709550[0-9]{3}|18446744073709551[0-5][0-9]{2}|1844674407370955160[0-9]{1}|1844674407370955161[0-4]|18446744073709551615)(?![\\s\\S])"));
    // This asserts actual emission and serde behavior. It does not execute an
    // ECMAScript/JSON Schema regex engine or claim consumer interoperability.
    let rows = [
        ("0", true), ("1", true), ("9007199254740992", true),
        ("9999999999999999999", true), ("18446744073709551614", true),
        ("18446744073709551615", true), ("18446744073709551616", false),
        ("99999999999999999999", false), ("100000000000000000000", false),
        ("", false), ("00", false), ("01", false), ("+1", false),
        ("-1", false), ("1.0", false), ("1e0", false), (" 1", false),
        ("1 ", false), ("١", false), ("１", false), ("x1", false),
        ("1x", false), ("1\n", false), ("1\r\n", false), ("1\r", false),
        ("1\u{2028}", false), ("1\u{2029}", false), ("1\0", false),
        ("1\t", false), ("\n1", false), ("1\n2", false),
    ];
    for (text, expected) in rows {
        let actual = serde_json::from_value::<Count>(json!(text));
        assert_eq!(actual.is_ok(), expected, "{text:?}");
        if let Ok(value) = actual {
            assert_eq!(serde_json::to_value(value).unwrap(), json!(text));
        }
    }
    for value in [json!(1), json!(1.0), json!(true), json!(null), json!([]), json!({})] {
        assert!(serde_json::from_value::<Count>(value).is_err());
    }
}
