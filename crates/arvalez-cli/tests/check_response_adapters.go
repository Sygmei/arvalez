package adapters

import (
	"context"
	"encoding/json"
	"reflect"
	"testing"

	sdk "example.com/sdk"
	"github.com/hashicorp/terraform-plugin-framework/types"
)

func TestNativeResponseConversions(t *testing.T) {
	ctx := context.Background()
	input := &sdk.WidgetResponse{}
	model, d := WidgetResponseModelFromAPI(ctx, input)
	if d.HasError() {
		t.Fatal(d)
	}
	if !model.Nickname.IsNull() || !model.Labels.IsNull() || !model.Metadata.IsNull() || !model.Choice.IsNull() || !model.Pair.IsNull() {
		t.Fatal("native null not preserved")
	}
	output, d := WidgetResponseModelToAPI(ctx, model)
	if d.HasError() {
		t.Fatal(d)
	}
	if !reflect.DeepEqual(input, output) {
		t.Fatalf("model round trip: %#v != %#v", input, output)
	}
	value, d := WidgetResponseValueFromAPI(ctx, input)
	if d.HasError() {
		t.Fatal(d)
	}
	attrs := value.Attributes()
	if _, ok := attrs["display_name"]; !ok {
		t.Fatal("source override not inherited")
	}
	if _, ok := attrs["amount"]; !ok {
		t.Fatal("variant override missing")
	}
	output, d = WidgetResponseValueToAPI(ctx, value)
	if d.HasError() || !reflect.DeepEqual(input, output) {
		t.Fatalf("value round trip: %#v %v", output, d)
	}
	input.WithNickname("").WithLabels([]string{}).WithMetadata(map[string]string{}).WithBlob([]byte{}).WithChoice("value").WithPair([2]any{"a", json.Number("2")})
	model, d = WidgetResponseModelFromAPI(ctx, input)
	if d.HasError() {
		t.Fatal(d)
	}
	if model.Nickname.IsNull() || model.Labels.IsNull() || model.Metadata.IsNull() || model.Choice.IsNull() {
		t.Fatal("empty values became null")
	}
	output, d = WidgetResponseModelToAPI(ctx, model)
	if d.HasError() || !reflect.DeepEqual(input, output) {
		t.Fatalf("empty round trip: %#v %v", output, d)
	}
	model.Nickname = types.StringUnknown()
	if _, d := WidgetResponseModelToAPI(ctx, model); !d.HasError() {
		t.Fatal("unknown value accepted")
	}
	if _, d := WidgetResponseValueToAPI(ctx, types.ObjectUnknown(WidgetResponseAttributeTypes())); !d.HasError() {
		t.Fatal("unknown object accepted")
	}
	value, d = WidgetResponseValueFromAPI(ctx, nil)
	if d.HasError() || !value.IsNull() {
		t.Fatal("nil root not preserved")
	}
}
