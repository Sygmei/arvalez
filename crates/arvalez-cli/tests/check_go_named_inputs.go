package client

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

type recordingTransport func(*http.Request) (*http.Response, error)

func (f recordingTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestNamedInputs(t *testing.T) {
	ctx := context.Background()
	var captured *http.Request
	var body string
	count := 0
	status := 200
	client := NewClient("https://example.test", &ClientOptions{Headers: http.Header{"X-Default": {"yes"}}, HTTPClient: &http.Client{
		Transport: recordingTransport(func(r *http.Request) (*http.Response, error) {
			count++
			captured = r
			body = ""
			if r.Body != nil {
				data, err := io.ReadAll(r.Body)
				if err != nil {
					t.Fatal(err)
				}
				body = string(data)
			}
			return &http.Response{StatusCode: status, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"id":"result"}`))}, nil
		}),
	}})
	// A model called GetWidgetParams must not collide with operation inputs.
	patch := (&Widget{ID: "sent"}).WithCount2(0).WithPictureURLNull()
	params := (&GetWidgetParams2{WidgetID: "a/b"}).WithIncludeCount(false).WithPage(0).
		WithFilter("").WithXLabel("").WithSession("abc").WithBody(*patch)
	calls := 0
	option := RequestOption(func(o *RequestOptions) { calls++; o.Headers = http.Header{"X-Option": {"yes"}} })
	result, err := client.GetWidget(ctx, params, option)
	if err != nil || result.ID != "result" {
		t.Fatalf("decoded call: %#v %v", result, err)
	}
	if calls != 1 {
		t.Fatalf("options applied %d times", calls)
	}
	if captured.URL.EscapedPath() != "/widgets/a%2Fb" {
		t.Fatal(captured.URL)
	}
	if captured.URL.Query().Get("page") != "0" || captured.URL.Query().Get("include_count") != "false" || !captured.URL.Query().Has("filter") {
		t.Fatal(captured.URL)
	}
	if _, present := captured.Header["X-Label"]; !present {
		t.Fatal("empty header omitted")
	}
	if cookie, err := captured.Cookie("session"); err != nil || cookie.Value != "abc" {
		t.Fatal(cookie, err)
	}
	if captured.Header.Get("X-Option") != "yes" || captured.Header.Get("X-Default") != "yes" {
		t.Fatal(captured.Header)
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(body), &payload); err != nil {
		t.Fatal(err)
	}
	if v, ok := payload["picture_url"]; !ok || v != nil {
		t.Fatal(payload)
	}
	if payload["count"] != float64(0) {
		t.Fatal(payload)
	}
	if params.IncludeCount == nil || *params.IncludeCount || params.Body.PictureURL == nil {
		t.Fatal("call mutated inputs")
	}

	params.WithoutIncludeCount().WithoutPage().WithoutFilter().WithoutXLabel().WithoutSession().WithoutBody()
	response, err := client.GetWidgetRaw(ctx, params)
	if err != nil {
		t.Fatal(err)
	}
	response.Body.Close()
	if captured.URL.RawQuery != "" || body != "" {
		t.Fatal(captured.URL, body)
	}
	if _, err := client.GetWidgetRaw(ctx, nil); err == nil {
		t.Fatal("nil required params accepted")
	}
	before := count
	if _, err := client.GetWidget(ctx, nil); err == nil || count != before {
		t.Fatal("invalid input reached transport")
	}
	// Required values are plain fields: empty string is supplied, not inferred missing.
	if _, err := client.GetWidget(ctx, &GetWidgetParams2{}); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Ping(ctx, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := client.ListWidgets(ctx, nil); err != nil {
		t.Fatal(err)
	}
	if captured.URL.RawQuery != "" {
		t.Fatal(captured.URL)
	}
	if _, err := client.ListWidgets(ctx, (&ListWidgetsParams{}).WithEnabled(false).WithPage(0).WithFilter("")); err != nil {
		t.Fatal(err)
	}
	if captured.URL.Query().Get("enabled") != "false" || captured.URL.Query().Get("page") != "0" || !captured.URL.Query().Has("filter") {
		t.Fatal(captured.URL)
	}

	first := RequestOptions{Headers: http.Header{"X-First": {"1"}}, Query: url.Values{"page": {"9"}}}
	second := RequestOptions{Headers: http.Header{"X-Second": {"2"}}}
	if _, err := client.ListWidgets(ctx, nil, WithRequestOptions(first), WithRequestOptions(second)); err != nil {
		t.Fatal(err)
	}
	if captured.Header.Get("X-First") != "" || captured.Header.Get("X-Second") != "2" || captured.URL.Query().Has("page") {
		t.Fatal("last option must replace prior settings")
	}
	if first.Headers.Get("X-First") != "1" {
		t.Fatal("mutated supplied options")
	}

	if _, err := client.RequiredBody(ctx, &RequiredBodyParams{Body: Widget{ID: "required"}}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(body, `"id":"required"`) {
		t.Fatal(body)
	}
	if _, err := client.UntypedBody(ctx, (&UntypedBodyParams{}).WithBody(nil)); err != nil {
		t.Fatal(err)
	}
	if body != "null" {
		t.Fatal(body)
	}
	if _, err := client.Stream(ctx, (&StreamParams{}).WithBody(strings.NewReader("bytes"))); err != nil {
		t.Fatal(err)
	}
	if body != "bytes" {
		t.Fatal(body)
	}
	if _, err := client.Multipart(ctx, (&MultipartParams{}).WithBody(Widget{ID: "form"})); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(captured.Header.Get("Content-Type"), "multipart/form-data") || !strings.Contains(body, "form") {
		t.Fatal(body)
	}

	patch.WithPictureURL("").WithoutCount().WithoutPictureURL()
	encoded, err := json.Marshal(patch)
	if err != nil || strings.Contains(string(encoded), "picture_url") || strings.Contains(string(encoded), `"count"`) {
		t.Fatal(string(encoded), err)
	}
	collision := (&CollisionParams{Body: "path"}).WithParams2("p").WithRequestOptions("r").WithAB("one").WithAB2("two").WithBody2(Widget{ID: "body"})
	if _, err := client.Collision(ctx, collision); err != nil {
		t.Fatal(err)
	}
	if captured.URL.Path != "/collision/path" || captured.URL.Query().Get("a-b") != "one" || captured.URL.Query().Get("a_b") != "two" {
		t.Fatal(captured.URL)
	}
	status = 400
	calls = 0
	response, err = client.GetWidgetRaw(ctx, params, option)
	if err != nil || response.StatusCode != 400 || calls != 1 {
		t.Fatal("raw response behavior", err)
	}
	response.Body.Close()
	handled := 0
	if _, err := client.GetWidget(ctx, params, WithRequestOptions(RequestOptions{OnError: func(*http.Response) error { handled++; return nil }})); err != nil || handled != 1 {
		t.Fatal(err, handled)
	}
}
