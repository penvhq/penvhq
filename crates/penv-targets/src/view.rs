//! The schema JSON a template sees: `penv schema --json` with three changes.

use std::collections::BTreeMap;

use penv_schema::resolve::{Raw, resolve_full, tainted};
use penv_schema::{Schema, Values};
use serde_json::Value;

/// A computed default is dropped and its key required, because a literal
/// fallback like `"random(48)"` would be a wrong value outside `penv run`; a key
/// computed from a secret is sensitive; each key says whether it is public.
/// `environment` is the one the computed defaults are read in.
pub fn view(schema: &Schema, environment: &str) -> Value {
    let raw: BTreeMap<String, Raw> = schema
        .keys
        .iter()
        .filter_map(|key| {
            let default = key.default.as_ref()?;
            let raw = if key.default_expr {
                Raw::computed(default.clone())
            } else {
                Raw::literal(default.clone())
            };
            (!(raw.computed && raw.text.trim_start().starts_with("random(")))
                .then(|| (key.name.clone(), raw))
        })
        .collect();
    let resolution = resolve_full(&raw, &Values::new(), environment, &Values::new());
    let hot = tainted(&resolution.deps, |name| {
        schema.get(name).is_none_or(|k| k.sensitive)
    });
    let mut json = schema.to_json();
    if let Some(keys) = json.get_mut("keys").and_then(|k| k.as_array_mut()) {
        for key in keys {
            let name = key["name"].as_str().unwrap_or_default().to_string();
            if key["defaultExpr"] == Value::Bool(true) {
                key["default"] = Value::Null;
                key["required"] = Value::Bool(true);
            }
            if hot.contains(&name) {
                key["sensitive"] = Value::Bool(true);
            }
            key["public"] = Value::Bool(schema.is_public(&name));
        }
    }
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed<'a>(json: &'a Value, name: &str) -> &'a Value {
        json["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == name)
            .unwrap()
    }

    #[test]
    fn a_computed_default_is_dropped_and_a_key_built_from_a_secret_is_one() {
        let schema = penv_schema::parse(
            "# @type=string\nSTRIPE_SECRET_KEY=\n\n# @type=string @sensitive=false\nNEXT_PUBLIC_TOKEN=${STRIPE_SECRET_KEY}\n\n# @type=string\nSESSION=random(32)\n\n# @type=port @sensitive=false\nPORT=3000\n",
        )
        .unwrap();
        let json = view(&schema, "development");
        let token = keyed(&json, "NEXT_PUBLIC_TOKEN");
        assert_eq!(token["default"], Value::Null);
        assert_eq!(token["required"], true);
        assert_eq!(token["sensitive"], true, "built from a secret");
        assert_eq!(token["public"], true);
        assert_eq!(keyed(&json, "SESSION")["default"], Value::Null);
        let port = keyed(&json, "PORT");
        assert_eq!(port["default"], "3000");
        assert_eq!(port["sensitive"], false);
        assert_eq!(port["public"], false);
    }
}
