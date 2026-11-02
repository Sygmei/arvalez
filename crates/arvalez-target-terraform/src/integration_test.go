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
	model, d := NullableInputModelFromAPI(ctx, input)
	check(t, d)
	if !model.RequiredValue.IsNull() || model.OptionalValue.IsNull() || model.ExplicitValue.IsNull() || model.Numbers.IsNull() || model.Labels.IsNull() {
		t.Fatal("nullable values, zero values, or empty collections lost")
	}
	output, d := NullableInputModelToAPI(ctx, model)
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
	output, d = NullableInputModelToAPI(ctx, &restored)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatal("state round trip changed nullable values")
	}
	// Required null stays explicit; optional null follows each configured policy.
	model.OptionalValue = types.StringNull()
	model.ExplicitValue = types.Int64Null()
	output, d = NullableInputModelToAPI(ctx, model)
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
	output, d = NullableInputModelToAPI(ctx, model)
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
	model, d := WidgetModelFromAPI(ctx, input)
	check(t, d)
	if model.Title.IsNull() || model.Title.ValueString() != "" || model.Count.IsNull() || model.Count.ValueInt64() != 0 {
		t.Fatal("zero values lost")
	}
	if model.Enabled.ValueBool() || model.Ratio.ValueFloat64() != 0 {
		t.Fatal("scalar mismatch")
	}
	output, d := WidgetModelToAPI(ctx, model)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("round trip mismatch:\nin %#v\nout %#v", input, output)
	}
	if !strings.Contains(model.Payload.ValueString(), "9007199254740993") {
		t.Fatal("JSON precision lost")
	}
	s := sdk.Status("ready")
	sm, d := StatusModelFromAPI(ctx, &s)
	check(t, d)
	back, d := StatusModelToAPI(ctx, sm)
	check(t, d)
	if *back != s {
		t.Fatal("enum mismatch")
	}
	if got, d := WidgetModelFromAPI(ctx, nil); got != nil || d.HasError() {
		t.Fatal("nil input")
	}
	if got, d := WidgetModelToAPI(ctx, nil); got != nil || d.HasError() {
		t.Fatal("nil input")
	}
}

func TestValueRoundTripAndFrameworkComposition(t *testing.T) {
	input := sample()
	value, d := WidgetValueFromAPI(ctx, input)
	check(t, d)
	output, d := WidgetValueToAPI(ctx, value)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("value round trip mismatch: %#v != %#v", input, output)
	}
	model, d := WidgetModelFromAPI(ctx, input)
	check(t, d)
	fromModel, d := types.ObjectValueFrom(ctx, WidgetAttributeTypes(), model)
	check(t, d)
	if !value.Equal(fromModel) {
		t.Fatal("model and value conversions disagree")
	}
	attrs, d := WidgetResourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	state := tfsdk.State{Schema: resourceschema.Schema{Attributes: attrs}}
	check(t, state.Set(ctx, value))
	var restored types.Object
	check(t, state.Get(ctx, &restored))
	if !restored.Equal(value) {
		t.Fatal("framework state changed the object")
	}
	// Native values can be used directly as nested attributes and collection elements.
	child, d := ChildValueFromAPI(ctx, input.Child)
	check(t, d)
	model.Child = child
	model.Children, d = types.ListValue(types.ObjectType{AttrTypes: ChildAttributeTypes()}, []attr.Value{child})
	check(t, d)
	model.Groups, d = types.MapValue(types.ObjectType{AttrTypes: ChildAttributeTypes()}, map[string]attr.Value{"first": child})
	check(t, d)
	output, d = WidgetModelToAPI(ctx, model)
	check(t, d)
	if !reflect.DeepEqual(output.Child, input.Child) || !reflect.DeepEqual(output.Children, []sdk.Child{*input.Child}) || !reflect.DeepEqual(output.Groups["first"], *input.Child) {
		t.Fatal("native values did not compose with the model")
	}
}

func TestValueNullUnknownAndInvalidTypes(t *testing.T) {
	null, d := WidgetValueFromAPI(ctx, nil)
	check(t, d)
	if !null.Equal(types.ObjectNull(WidgetAttributeTypes())) {
		t.Fatal("nil input did not produce a correctly typed null")
	}
	if output, d := WidgetValueToAPI(ctx, null); output != nil || d.HasError() {
		t.Fatalf("null input: %v %v", output, d)
	}
	wrongType := map[string]attr.Type{"wrong": types.StringType}
	for _, invalid := range []types.Object{
		types.ObjectUnknown(WidgetAttributeTypes()),
		types.ObjectNull(wrongType),
		types.ObjectUnknown(wrongType),
		types.ObjectValueMust(wrongType, map[string]attr.Value{"wrong": types.StringValue("x")}),
	} {
		if output, d := WidgetValueToAPI(ctx, invalid); output != nil || !d.HasError() {
			t.Fatalf("invalid object accepted: %v %v", output, d)
		}
	}
	value, d := WidgetValueFromAPI(ctx, sample())
	check(t, d)
	attributes := value.Attributes()
	attributes["child"] = types.ObjectValueMust(ChildAttributeTypes(), map[string]attr.Value{
		"action": types.StringUnknown(), "score": types.Int64Null(),
	})
	value, d = types.ObjectValue(WidgetAttributeTypes(), attributes)
	check(t, d)
	output, d := WidgetValueToAPI(ctx, value)
	if output != nil || !d.HasError() {
		t.Fatal("nested unknown value accepted")
	}
	diagnostic, ok := d[0].(diag.DiagnosticWithPath)
	if !ok || !diagnostic.Path().Equal(path.Root("child").AtName("action")) {
		t.Fatalf("missing nested path: %v", d)
	}
	for _, mutate := range []func(*sdk.Widget){
		func(w *sdk.Widget) { w.Ratio = math.Inf(1) },
		func(w *sdk.Widget) { w.Payload = make(chan int) },
	} {
		input := sample()
		mutate(input)
		value, d := WidgetValueFromAPI(ctx, input)
		if !value.Equal(null) || !d.HasError() {
			t.Fatalf("invalid API input did not return typed null and diagnostics: %v %v", value, d)
		}
	}
}

func TestScalarValueConversions(t *testing.T) {
	input := sdk.Status("ready")
	value, d := StatusValueFromAPI(ctx, &input)
	check(t, d)
	if !value.Equal(types.StringValue("ready")) {
		t.Fatal("enum conversion lost its value")
	}
	output, d := StatusValueToAPI(ctx, value)
	check(t, d)
	if output == nil || *output != input {
		t.Fatal("enum round trip mismatch")
	}
	null, d := StatusValueFromAPI(ctx, nil)
	check(t, d)
	if !null.IsNull() {
		t.Fatal("nil enum did not produce null")
	}
	if output, d := StatusValueToAPI(ctx, null); output != nil || d.HasError() {
		t.Fatalf("null enum: %v %v", output, d)
	}
	if output, d := StatusModelToAPI(ctx, &null); output != nil || d.HasError() {
		t.Fatalf("null enum model: %v %v", output, d)
	}
	if output, d := StatusValueToAPI(ctx, types.StringUnknown()); output != nil || !d.HasError() {
		t.Fatal("unknown enum accepted")
	}
}

func TestNullAndEmpty(t *testing.T) {
	input := sample()
	input.Title = nil
	input.Nullable = sdk.Null[string]()
	input.Labels = nil
	input.Children = nil
	input.Blob = nil
	model, d := WidgetModelFromAPI(ctx, input)
	check(t, d)
	if !model.Title.IsNull() || !model.Nullable.IsNull() || !model.Labels.IsNull() || !model.Children.IsNull() || !model.Blob.IsNull() {
		t.Fatal("nil values lost")
	}
	output, d := WidgetModelToAPI(ctx, model)
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
	model, d = WidgetModelFromAPI(ctx, input)
	check(t, d)
	output, d = WidgetModelToAPI(ctx, model)
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
			m, d := WidgetModelFromAPI(ctx, sample())
			check(t, d)
			mutate(m)
			output, d := WidgetModelToAPI(ctx, m)
			if output != nil || !d.HasError() {
				t.Fatalf("expected no output and diagnostic, got %v %v", output, d)
			}
		})
	}
	m, d := WidgetModelFromAPI(ctx, sample())
	check(t, d)
	m.Child = types.ObjectValueMust(ChildAttributeTypes(), map[string]attr.Value{"action": types.StringUnknown(), "score": types.Int64Null()})
	_, d = WidgetModelToAPI(ctx, m)
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
	m, d := WidgetModelFromAPI(ctx, sample())
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
	cm, d := ChildModelFromAPI(ctx, &sdk.Child{Action: "test"})
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
	_, d = WidgetModelToAPI(ctx, &plan)
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
	m, d := WidgetModelFromAPI(ctx, input)
	check(t, d)
	output, d := WidgetModelToAPI(ctx, m)
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
	if output, d := WidgetModelFromAPI(ctx, input); output != nil || !d.HasError() {
		t.Fatal("invalid JSON API value accepted")
	}
	input = sample()
	input.Ratio = math.Inf(1)
	if output, d := WidgetModelFromAPI(ctx, input); output != nil || !d.HasError() {
		t.Fatal("non-finite API number accepted")
	}
	status := types.StringUnknown()
	if output, d := StatusModelToAPI(ctx, &status); output != nil || !d.HasError() {
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
	model, d := EnvelopeModelFromAPI(ctx, input)
	check(t, d)
	if model.Node.IsNull() || model.Choices.IsNull() || !strings.Contains(model.Choice.ValueString(), "9007199254740993") {
		t.Fatal("automatic representation lost a value")
	}
	output, d := EnvelopeModelToAPI(ctx, model)
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
	output, d = EnvelopeModelToAPI(ctx, &restored)
	check(t, d)
	if !reflect.DeepEqual(input, output) {
		t.Fatal("automatic JSON state changed values")
	}
	ds, d := EnvelopeDataSourceAttributes(ctx, SchemaOptions{})
	check(t, d)
	check(t, (datasourceschema.Schema{Attributes: ds}).ValidateImplementation(ctx))
	for _, value := range []types.String{types.StringUnknown(), types.StringValue("[broken"), types.StringValue(`{"not":"an array"}`)} {
		model.Choices = value
		output, d := EnvelopeModelToAPI(ctx, model)
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
	if output, d := CycleAModelFromAPI(ctx, cycle); output != nil || !d.HasError() {
		t.Fatal("cyclic Go value accepted")
	}
}
