use anyhow::Result;
use arvalez_ir::{CoreIr, Field, Model, TypeRef};
use arvalez_plugin_sdk::{Plugin, PluginContext, TransformOutput, run_plugin};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct MoneyOptions {
    #[serde(default = "default_currency_type")]
    currency_type: String,
}

impl Default for MoneyOptions {
    fn default() -> Self {
        Self {
            currency_type: default_currency_type(),
        }
    }
}

struct MoneyPlugin;

impl Plugin for MoneyPlugin {
    type Options = MoneyOptions;

    fn transform_core(
        &self,
        ctx: &PluginContext<Self::Options>,
        mut ir: CoreIr,
    ) -> Result<TransformOutput> {
        if !ir.models.iter().any(|model| model.name == "Money") {
            let mut money = Model::new("model.money", "Money");
            money
                .fields
                .push(Field::new("amount", TypeRef::named("Decimal")));
            money.fields.push(Field::new(
                "currency",
                TypeRef::named(ctx.options().currency_type.clone()),
            ));
            ir.models.push(money);
        }

        let mut rewrites = 0usize;
        for model in &mut ir.models {
            if model.name == "Money" {
                continue;
            }

            for field in &mut model.fields {
                let looks_like_money = matches!(field.name.as_str(), "total" | "price" | "amount");
                let is_decimal = field.type_ref == TypeRef::named("Decimal");
                if looks_like_money && is_decimal {
                    field.type_ref = TypeRef::named("Money");
                    rewrites += 1;
                }
            }
        }

        let mut output = TransformOutput::ok(ir);
        if rewrites > 0 {
            output = output.with_warning(format!(
                "plugin `{}` rewrote {rewrites} decimal money field(s)",
                ctx.plugin_name()
            ));
        }

        Ok(output)
    }
}

fn main() -> Result<()> {
    run_plugin(MoneyPlugin)
}

fn default_currency_type() -> String {
    "CurrencyCode".into()
}
