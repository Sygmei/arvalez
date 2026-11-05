import type { Widget, WidgetResponse, Problem } from "./models";

const input: Widget = { id: 1 };
const output: WidgetResponse = {
  id: 1, title: "", nickname: null, enabled: false, quantity: 0,
  labels: [], metadata: {}, blob: null, state: null, child: null,
  partial: {}, choice: null, pair: null,
};
const error: Problem = { message: null };
// @ts-expect-error Response fields must be present.
const missing: WidgetResponse = { id: 1 };
// @ts-expect-error Non-nullable response field cannot be null.
const invalid: WidgetResponse = { ...output, title: null };
void [input, output, error, missing, invalid];
