use std::{env, fs, path::PathBuf};

use serde_json::Value;

const TARGET_SCHEMA_PATH: &str = "schemas/v1/target.schema.json";
const TARGET_SCHEMA_URN: &str = "urn:repeto:schema:v1:target";

fn main() {
    const SCHEMAS: [&str; 4] = [
        "schemas/v1/config.schema.json",
        "schemas/v1/target.schema.json",
        "schemas/v1/event.schema.json",
        "schemas/v1/review-record-input.schema.json",
    ];

    let mut type_space = typify::TypeSpace::new(&typify::TypeSpaceSettings::default());
    let target_schema = read_schema(TARGET_SCHEMA_PATH);
    for schema_path in SCHEMAS {
        println!("cargo::rerun-if-changed={schema_path}");
        if schema_path == TARGET_SCHEMA_PATH {
            continue;
        }
        let mut schema = read_schema(schema_path);
        if schema_path == "schemas/v1/event.schema.json" {
            bundle_target_schema_for_typify(&mut schema, &target_schema);
        }
        type_space
            .add_root_schema(serde_json::from_value(schema).unwrap_or_else(|error| {
                panic!("could not prepare {schema_path} for type generation: {error}")
            }))
            .unwrap_or_else(|error| panic!("could not generate types for {schema_path}: {error}"));
    }

    let output = type_space.to_stream().to_string();
    let output_path = PathBuf::from(env::var("OUT_DIR").expect("Cargo always sets OUT_DIR"))
        .join("schema_types.rs");
    fs::write(output_path, output).expect("could not write generated schema types");
}

fn read_schema(schema_path: &str) -> Value {
    let schema_text = fs::read_to_string(schema_path)
        .unwrap_or_else(|error| panic!("could not read {schema_path}: {error}"));
    serde_json::from_str(&schema_text)
        .unwrap_or_else(|error| panic!("could not parse {schema_path}: {error}"))
}

fn bundle_target_schema_for_typify(event_schema: &mut Value, target_schema: &Value) {
    replace_target_references(event_schema);
    let definitions = event_schema
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("event schema must define $defs"));
    definitions.insert("RepetoTargetDefinition".to_owned(), target_schema.clone());
}

fn replace_target_references(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("$ref") == Some(&Value::String(TARGET_SCHEMA_URN.to_owned())) {
                object.insert(
                    "$ref".to_owned(),
                    Value::String("#/$defs/RepetoTargetDefinition".to_owned()),
                );
            }
            for child in object.values_mut() {
                replace_target_references(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                replace_target_references(child);
            }
        }
        _ => {}
    }
}
