package sdk

import (
	"encoding/json"
	"reflect"
	"testing"
)

func TestResponseFields(t *testing.T) {
	var response WidgetResponse
	if err := json.Unmarshal([]byte(`{}`), &response); err != nil {
		t.Fatal(err)
	}
	body, err := json.Marshal(response)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(body, &fields); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"id", "title", "nickname", "enabled", "quantity", "labels", "metadata", "blob", "state", "child", "partial", "choice", "pair"} {
		if _, ok := fields[key]; !ok {
			t.Fatalf("omitted %s: %s", key, body)
		}
	}
	for _, key := range []string{"nickname", "labels", "metadata", "blob", "state", "child", "choice", "pair"} {
		if string(fields[key]) != "null" {
			t.Fatalf("%s: expected null, got %s", key, fields[key])
		}
	}
	if string(fields["enabled"]) != "false" || string(fields["quantity"]) != "0" {
		t.Fatal(string(body))
	}
	response.WithNickname("").WithLabels([]string{}).WithMetadata(map[string]string{}).WithBlob([]byte{}).WithState(State("ready")).WithPair([2]any{"a", float64(2)}).WithChoice("value")
	body, err = json.Marshal(response)
	if err != nil {
		t.Fatal(err)
	}
	var decoded WidgetResponse
	if err := json.Unmarshal(body, &decoded); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(response, decoded) {
		t.Fatalf("round trip: %#v != %#v", response, decoded)
	}
	if response.Nickname == nil || *response.Nickname != "" || response.Labels == nil || response.Metadata == nil {
		t.Fatal("empty values lost")
	}
	response.WithNicknameNull().WithLabelsNull().WithMetadataNull().WithPairNull().WithChoiceNull()
	if response.Nickname != nil || response.Labels != nil || response.Metadata != nil || response.Pair != nil || response.Choice != nil {
		t.Fatal("null setter failed")
	}
}

func TestRequestPreservesThreeStates(t *testing.T) {
	request := Widget{}
	check := func(want string, present bool) {
		t.Helper()
		data, err := json.Marshal(request)
		if err != nil {
			t.Fatal(err)
		}
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(data, &fields); err != nil {
			t.Fatal(err)
		}
		raw, ok := fields["nickname"]
		if ok != present || string(raw) != want {
			t.Fatalf("nickname=%s present=%v", raw, ok)
		}
	}
	check("", false)
	request.WithNicknameNull()
	check("null", true)
	request.WithNickname("")
	check(`""`, true)
	request.WithoutNickname()
	check("", false)
}

// Making optional object fields required must not create infinitely sized structs.
func TestRecursiveResponse(t *testing.T) {
	var value Link
	if err := json.Unmarshal([]byte(`{"next":{}}`), &value); err != nil {
		t.Fatal(err)
	}
	if value.Next == nil || value.Next.Next != nil {
		t.Fatal("recursive response did not decode")
	}
}
