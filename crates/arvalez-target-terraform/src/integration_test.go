package tfmodels

import (
	"context"
	"encoding/json"
	"math"
	"reflect"
	"strings"
	"testing"

	sdk "example.com/sdk/v2"
	"github.com/hashicorp/terraform-plugin-framework/attr"
	datasourceschema "github.com/hashicorp/terraform-plugin-framework/datasource/schema"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/path"
	resourceschema "github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/tfsdk"
	"github.com/hashicorp/terraform-plugin-framework/types"
)

var ctx = context.Background()

func check(t *testing.T, d diag.Diagnostics) {
	t.Helper()
	if d.HasError() {
		t.Fatalf("diagnostics: %v", d)
	}
}
func sample() *sdk.Widget {
	title := ""
	count := int64(0)
	status := sdk.Status("ready")
	return &sdk.Widget{Action: "create", Title: &title, Count: &count, Status: &status,
		Child: &sdk.Child{Action: "child"}, Children: []sdk.Child{{Action: "nested"}},
		Labels: map[string]string{}, Groups: map[string]sdk.Child{"first": {Action: "group"}},
		Matrix: [][]int64{{1, 2}, nil, {}}, Blob: []byte{},
		Nullable: sdk.NewNullable(""), Clear: sdk.Null[int64](),
		RequiredNullable: *sdk.Null[string](),
		Payload:          map[string]any{"large": json.Number("9007199254740993")},
		Tuple:            [2]any{"a", json.Number("2")},
	}
}

// These SDK and Terraform models are both generated from OpenAPI 3.0 input.
func TestOpenAPI30Nullable(t *testing.T) {
	input := &sdk.NullableInput{
		RequiredValue: *sdk.Null[string](),
		OptionalValue: sdk.NewNullable(""),
		ExplicitValue: sdk.NewNullable(int64(0)),
		Numbers:       sdk.NewNullable([]int64{}),
		Labels:        sdk.NewNullable(map[string]string{}),
		Child:         sdk.NewNullable(sdk.NullableChild{Action: "create"}),
		State:         sdk.NewNullable(sdk.NullableState("ready")),
	}
	model, d := NullableInputFromAPI(ctx, input)
	check(t, d)
	if !model.RequiredValue.IsNull() || model.OptionalValue.IsNull() || model.ExplicitValue.IsNull() || model.Numbers.IsNull() || model.Labels.IsNull() {
		t.Fatal("nullable values, zero values, or empty collections lost")
	}
	output, d := NullableInputToAPI(ctx, model)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("nullable round trip mismatch: %#v != %#v", input, output)
	}
	attrs, d := NullableInputResourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	state := tfsdk.State{Schema: resourceschema.Schema{Attributes: attrs}}
	check(t, state.Set(ctx, model))
	var restored NullableInputModel
	check(t, state.Get(ctx, &restored))
	output, d = NullableInputToAPI(ctx, &restored)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatal("state round trip changed nullable values")
	}
	// Required null stays explicit; optional null follows each configured policy.
	model.OptionalValue = types.StringNull()
	model.ExplicitValue = types.Int64Null()
	output, d = NullableInputToAPI(ctx, model)
	check(t, d)
	if output.RequiredValue.Valid || output.OptionalValue != nil || output.ExplicitValue == nil || output.ExplicitValue.Valid {
		t.Fatal("incorrect omission/explicit null policy")
	}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	var body map[string]json.RawMessage
	if err := json.Unmarshal(encoded, &body); err != nil {
		t.Fatal(err)
	}
	if string(body["requiredValue"]) != "null" || string(body["explicitValue"]) != "null" {
		t.Fatalf("missing explicit JSON null: %s", encoded)
	}
	if _, exists := body["optionalValue"]; exists {
		t.Fatal("optional value was not omitted")
	}
	model.RequiredValue = types.StringUnknown()
	output, d = NullableInputToAPI(ctx, model)
	if output != nil || !d.HasError() {
		t.Fatal("unknown nullable value accepted")
	}
	diagnostic, ok := d[0].(diag.DiagnosticWithPath)
	if !ok || !diagnostic.Path().Equal(path.Root("required_value")) {
		t.Fatalf("missing nullable field path: %v", d)
	}
}
func TestRoundTrip(t *testing.T) {
	input := sample()
	model, d := WidgetFromAPI(ctx, input)
	check(t, d)
	if model.Title.IsNull() || model.Title.ValueString() != "" || model.Count.IsNull() || model.Count.ValueInt64() != 0 {
		t.Fatal("zero values lost")
	}
	if model.Enabled.ValueBool() || model.Ratio.ValueFloat64() != 0 {
		t.Fatal("scalar mismatch")
	}
	output, d := WidgetToAPI(ctx, model)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("round trip mismatch:\nin %#v\nout %#v", input, output)
	}
	if !strings.Contains(model.Payload.ValueString(), "9007199254740993") {
		t.Fatal("JSON precision lost")
	}
	s := sdk.Status("ready")
	sm, d := StatusFromAPI(ctx, &s)
	check(t, d)
	back, d := StatusToAPI(ctx, sm)
	check(t, d)
	if *back != s {
		t.Fatal("enum mismatch")
	}
	if got, d := WidgetFromAPI(ctx, nil); got != nil || d.HasError() {
		t.Fatal("nil input")
	}
	if got, d := WidgetToAPI(ctx, nil); got != nil || d.HasError() {
		t.Fatal("nil input")
	}
}
func TestNullAndEmpty(t *testing.T) {
	input := sample()
	input.Title = nil
	input.Nullable = sdk.Null[string]()
	input.Labels = nil
	input.Children = nil
	input.Blob = nil
	model, d := WidgetFromAPI(ctx, input)
	check(t, d)
	if !model.Title.IsNull() || !model.Nullable.IsNull() || !model.Labels.IsNull() || !model.Children.IsNull() || !model.Blob.IsNull() {
		t.Fatal("nil values lost")
	}
	output, d := WidgetToAPI(ctx, model)
	check(t, d)
	if output.Nullable != nil || output.Clear == nil || output.Clear.Valid || output.RequiredNullable.Valid {
		t.Fatal("null policies")
	}
	if output.Labels != nil || output.Children != nil || output.Blob != nil {
		t.Fatal("nil collections")
	}
	input.Labels = map[string]string{}
	input.Children = []sdk.Child{}
	input.Blob = []byte{}
	model, d = WidgetFromAPI(ctx, input)
	check(t, d)
	output, d = WidgetToAPI(ctx, model)
	check(t, d)
	if output.Labels == nil || output.Children == nil || output.Blob == nil {
		t.Fatal("empty collections became nil")
	}
}
func TestInvalidOutboundValues(t *testing.T) {
	cases := map[string]func(*WidgetModel){
		"unknown scalar":   func(m *WidgetModel) { m.Action = types.StringUnknown() },
		"required null":    func(m *WidgetModel) { m.Action = types.StringNull() },
		"optional unknown": func(m *WidgetModel) { m.Title = types.StringUnknown() },
		"nullable unknown": func(m *WidgetModel) { m.Clear = types.Int64Unknown() },
		"unknown object":   func(m *WidgetModel) { m.Child = types.ObjectUnknown(ChildAttributeTypes()) },
		"unknown list": func(m *WidgetModel) {
			m.Children = types.ListUnknown(types.ObjectType{AttrTypes: ChildAttributeTypes()})
		},
		"null list item": func(m *WidgetModel) {
			m.Children = types.ListValueMust(types.ObjectType{AttrTypes: ChildAttributeTypes()}, []attr.Value{types.ObjectNull(ChildAttributeTypes())})
		},
		"unknown list element": func(m *WidgetModel) {
			m.Children = types.ListValueMust(types.ObjectType{AttrTypes: ChildAttributeTypes()}, []attr.Value{types.ObjectUnknown(ChildAttributeTypes())})
		},
		"nested list unknown": func(m *WidgetModel) {
			m.Matrix = types.ListValueMust(types.ListType{ElemType: types.Int64Type}, []attr.Value{types.ListValueMust(types.Int64Type, []attr.Value{types.Int64Unknown()})})
		},
		"unknown object map value": func(m *WidgetModel) {
			m.Groups = types.MapValueMust(types.ObjectType{AttrTypes: ChildAttributeTypes()}, map[string]attr.Value{"x": types.ObjectUnknown(ChildAttributeTypes())})
		},
		"unknown map value": func(m *WidgetModel) {
			m.Labels = types.MapValueMust(types.StringType, map[string]attr.Value{"x": types.StringUnknown()})
		},
		"null map value": func(m *WidgetModel) {
			m.Labels = types.MapValueMust(types.StringType, map[string]attr.Value{"x": types.StringNull()})
		},
		"nested unknown": func(m *WidgetModel) {
			m.Child = types.ObjectValueMust(ChildAttributeTypes(), map[string]attr.Value{"action": types.StringUnknown(), "score": types.Int64Null()})
		},
		"non-finite number":            func(m *WidgetModel) { m.Ratio = types.Float64Value(math.Inf(1)) },
		"bad JSON":                     func(m *WidgetModel) { m.Payload = types.StringValue("{") },
		"trailing JSON":                func(m *WidgetModel) { m.Payload = types.StringValue("{} {}") },
		"JSON null":                    func(m *WidgetModel) { m.Payload = types.StringValue("null") },
		"bad base64":                   func(m *WidgetModel) { m.Blob = types.StringValue("!") },
		"truncated tuple":              func(m *WidgetModel) { m.Tuple = types.StringValue(`[1,2,3]`) },
		"wrong object shape":           func(m *WidgetModel) { m.Child = types.ObjectNull(map[string]attr.Type{"wrong": types.StringType}) },
		"recursive JSON unknown field": func(m *WidgetModel) { m.Next = types.StringValue(`{"unknown":1}`) },
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			m, d := WidgetFromAPI(ctx, sample())
			check(t, d)
			mutate(m)
			output, d := WidgetToAPI(ctx, m)
			if output != nil || !d.HasError() {
				t.Fatalf("expected no output and diagnostic, got %v %v", output, d)
			}
		})
	}
	m, d := WidgetFromAPI(ctx, sample())
	check(t, d)
	m.Child = types.ObjectValueMust(ChildAttributeTypes(), map[string]attr.Value{"action": types.StringUnknown(), "score": types.Int64Null()})
	_, d = WidgetToAPI(ctx, m)
	diagnostic, ok := d[0].(diag.DiagnosticWithPath)
	if !ok || !diagnostic.Path().Equal(path.Root("child").AtName("action")) {
		t.Fatalf("missing nested path: %v", d)
	}
}
func TestFrameworkStateAndSchemas(t *testing.T) {
	attrs, d := WidgetResourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	if !attrs["action"].IsComputed() {
		t.Fatal("default not computed")
	}
	schema := resourceschema.Schema{Attributes: attrs}
	check(t, schema.ValidateImplementation(ctx))
	m, d := WidgetFromAPI(ctx, sample())
	check(t, d)
	state := tfsdk.State{Schema: schema}
	check(t, state.Set(ctx, m))
	var restored WidgetModel
	check(t, state.Get(ctx, &restored))
	if !restored.Children.Equal(m.Children) {
		t.Fatal("nested list state changed")
	}
	// Reproduce the report: SDK objects themselves have no tfsdk Action tag.
	_, bad := types.ListValueFrom(ctx, types.ObjectType{AttrTypes: ChildAttributeTypes()}, []sdk.Child{{Action: "test"}})
	if !bad.HasError() || !strings.Contains(bad[0].Detail(), "tfsdk") {
		t.Fatalf("expected original failure: %v", bad)
	}
	cm, d := ChildFromAPI(ctx, &sdk.Child{Action: "test"})
	check(t, d)
	_, d = types.ListValueFrom(ctx, types.ObjectType{AttrTypes: ChildAttributeTypes()}, []ChildModel{*cm})
	check(t, d)
	sensitive := true
	options := SchemaOptions{Fields: map[string]FieldOptions{
		"action": {Role: Required, Sensitive: &sensitive},
		"title":  {Role: Optional},
		"child":  {Role: Optional, Fields: map[string]FieldOptions{"action": {Role: Required}}},
	}}
	attrs, d = WidgetResourceAttributes(ctx, options)
	check(t, d)
	check(t, (resourceschema.Schema{Attributes: attrs}).ValidateImplementation(ctx))
	if !attrs["action"].IsRequired() || !attrs["action"].IsSensitive() {
		t.Fatal("overrides")
	}
	if attrs["child"].(resourceschema.SingleNestedAttribute).Attributes["action"].GetDescription() == "" {
		t.Fatal("description lost")
	}
	ds, d := WidgetDataSourceAttributes(ctx, options)
	check(t, d)
	check(t, (datasourceschema.Schema{Attributes: ds}).ValidateImplementation(ctx))
	dsState := tfsdk.State{Schema: datasourceschema.Schema{Attributes: ds}}
	check(t, dsState.Set(ctx, m))
	config := tfsdk.Config{Schema: resourceschema.Schema{Attributes: attrs}, Raw: state.Raw}
	var plan WidgetModel
	check(t, config.Get(ctx, &plan))
	_, d = WidgetToAPI(ctx, &plan)
	check(t, d)
	_, d = WidgetResourceAttributes(ctx, SchemaOptions{Fields: map[string]FieldOptions{"title": {Role: OptionalComputed}}})
	check(t, d)
	badOptions := []SchemaOptions{
		{Fields: map[string]FieldOptions{"missing": {}}},
		{Fields: map[string]FieldOptions{"action": {Role: "invalid"}}},
		{Fields: map[string]FieldOptions{"action": {Fields: map[string]FieldOptions{"missing": {}}}}},
		{Fields: map[string]FieldOptions{"child": {Fields: map[string]FieldOptions{"missing": {}}}}},
	}
	for _, o := range badOptions {
		a, d := WidgetResourceAttributes(ctx, o)
		if a != nil || !d.HasError() {
			t.Fatal("accepted invalid options")
		}
	}
	a, d := WidgetDataSourceAttributes(ctx, SchemaOptions{Fields: map[string]FieldOptions{"title": {Role: OptionalComputed}}})
	if a != nil || !d.HasError() {
		t.Fatal("accepted resource-only role")
	}
}

func TestAutomaticJSONAndAPIErrorPaths(t *testing.T) {
	input := sample()
	input.Next = &sdk.Widget{Action: "next", Tuple: [2]any{"x", json.Number("3")}}
	m, d := WidgetFromAPI(ctx, input)
	check(t, d)
	output, d := WidgetToAPI(ctx, m)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatal("recursive JSON roundtrip")
	}
	_, d = jsonCodec[float64]().to(ctx, types.StringValue("1.0"), path.Root("number"))
	check(t, d)
	_, d = jsonCodec[float64]().to(ctx, types.StringValue("9007199254740993"), path.Root("number"))
	if !d.HasError() {
		t.Fatal("lossy float conversion accepted")
	}
	input.Payload = make(chan int)
	if output, d := WidgetFromAPI(ctx, input); output != nil || !d.HasError() {
		t.Fatal("invalid JSON API value accepted")
	}
	input = sample()
	input.Ratio = math.Inf(1)
	if output, d := WidgetFromAPI(ctx, input); output != nil || !d.HasError() {
		t.Fatal("non-finite API number accepted")
	}
	status := types.StringUnknown()
	if output, d := StatusToAPI(ctx, &status); output != nil || !d.HasError() {
		t.Fatal("unknown enum accepted")
	}
}

func TestAutomaticJSONForCyclesAndCollections(t *testing.T) {
	input := &sdk.Envelope{
		Node:    &sdk.CycleA{Label: "root", B: &sdk.CycleB{A: &sdk.CycleA{Label: "back"}}, Children: []sdk.CycleA{}},
		Choices: []any{"text", json.Number("9007199254740993")},
		Extras:  map[string]any{"large": json.Number("9007199254740993")},
		Choice:  json.Number("9007199254740993"),
	}
	model, d := EnvelopeFromAPI(ctx, input)
	check(t, d)
	if model.Node.IsNull() || model.Choices.IsNull() || !strings.Contains(model.Choice.ValueString(), "9007199254740993") {
		t.Fatal("automatic representation lost a value")
	}
	output, d := EnvelopeToAPI(ctx, model)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("automatic JSON round trip: %#v != %#v", input, output)
	}
	for name, typ := range CycleAAttributeTypes() {
		if !typ.Equal(types.StringType) {
			t.Fatalf("expected scalar or JSON for CycleA.%s: %v", name, typ)
		}
	}
	if _, ok := EnvelopeAttributeTypes()["node"].(types.ObjectType); !ok {
		t.Fatal("nonrecursive parent lost its typed object")
	}
	attrs, d := EnvelopeResourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	schema := resourceschema.Schema{Attributes: attrs}
	check(t, schema.ValidateImplementation(ctx))
	state := tfsdk.State{Schema: schema}
	check(t, state.Set(ctx, model))
	var restored EnvelopeModel
	check(t, state.Get(ctx, &restored))
	output, d = EnvelopeToAPI(ctx, &restored)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatal("automatic JSON state changed values")
	}
	ds, d := EnvelopeDataSourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	check(t, (datasourceschema.Schema{Attributes: ds}).ValidateImplementation(ctx))
	for _, value := range []types.String{types.StringUnknown(), types.StringValue("[broken"), types.StringValue(`{"not":"an array"}`)} {
		model.Choices = value
		output, d := EnvelopeToAPI(ctx, model)
		if output != nil || !d.HasError() {
			t.Fatal("invalid automatic JSON accepted")
		}
		diagnostic, ok := d[0].(diag.DiagnosticWithPath)
		if !ok || !diagnostic.Path().Equal(path.Root("choices")) {
			t.Fatalf("missing JSON field path: %v", d)
		}
	}
	cycle := &sdk.CycleA{Label: "cycle"}
	cycle.B = &sdk.CycleB{A: cycle}
	if output, d := CycleAFromAPI(ctx, cycle); output != nil || !d.HasError() {
		t.Fatal("cyclic Go value accepted")
	}
}
