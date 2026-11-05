package client

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"reflect"
	"strconv"
	"strings"
	"testing"
)

type paginationTransport func(*http.Request) (*http.Response, error)

func (f paginationTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type trackedBody struct {
	io.Reader
	closed *int
}

func (b *trackedBody) Close() error { *b.closed++; return nil }

func paginationClient(t *testing.T, fn func(*http.Request, int) (int, string)) (*Client, *int, *int) {
	t.Helper()
	calls, closed := 0, 0
	client := NewClient("https://example.test", &ClientOptions{Headers: http.Header{"X-Auth": []string{"secret"}}, HTTPClient: &http.Client{
		Transport: paginationTransport(func(r *http.Request) (*http.Response, error) {
			calls++
			if calls > 10 {
				t.Fatal("paginator did not stop")
			}
			if r.Header.Get("X-Auth") != "secret" {
				t.Fatal("lost authentication")
			}
			status, body := fn(r, calls)
			return &http.Response{StatusCode: status, Header: http.Header{}, Body: &trackedBody{strings.NewReader(body), &closed}}, nil
		}),
	}})
	return client, &calls, &closed
}

func TestPaginationStrategies(t *testing.T) {
	for _, name := range []string{"zero", "one", "total", "last", "more", "short", "empty", "offset", "cursor", "root", "nested"} {
		t.Run(name, func(t *testing.T) {
			client, calls, closed := paginationClient(t, func(r *http.Request, n int) (int, string) {
				q := r.URL.Query()
				if q.Get("limit") != "2" {
					t.Fatalf("page size: %v", q)
				}
				if name == "cursor" {
					if n == 1 && q.Has("cursor") {
						t.Fatal("first request must omit cursor")
					}
					if n == 2 && q.Get("cursor") != "token/+=?" {
						t.Fatalf("cursor: %v", q)
					}
				} else if name == "offset" {
					// Server returns fewer items than requested: advance by actual count.
					if q.Get("offset") != strconv.Itoa(n+1) {
						t.Fatalf("offset: %v", q)
					}
				} else {
					expected := n
					if name == "zero" || name == "last" {
						expected--
					}
					if q.Get("page") != strconv.Itoa(expected) {
						t.Fatalf("page: %v", q)
					}
				}
				items := fmt.Sprintf(`[{"id":"%d"}]`, n)
				if (name == "short" || name == "empty" || name == "root") && n == 1 {
					items = `[{"id":"1"},{"id":"2"}]`
				}
				if (name == "empty" || name == "root") && n == 2 {
					items = `[]`
				}
				if name == "root" {
					return 200, items
				}
				cursor := `"token/+=?"`
				if n == 2 {
					cursor = `null`
				}
				body := fmt.Sprintf(`{"items":%s,"pages":2,"total":%d,"last":1,"more":%t,"next":%s}`, items, map[bool]int{false: 2, true: 4}[name == "offset"], n < 2, cursor)
				if name == "nested" {
					body = `{"a/b":` + body + `}`
				}
				return 200, body
			})
			var items []Widget
			var err error
			switch name {
			case "zero":
				items, err = client.API.PageZeroCollect(context.Background(), nil)
			case "one":
				items, err = client.API.PageOneCollect(context.Background(), nil)
			case "total":
				items, err = client.API.TotalItemsCollect(context.Background(), nil)
			case "last":
				items, err = client.API.LastPageCollect(context.Background(), nil)
			case "more":
				items, err = client.API.MoreCollect(context.Background(), nil)
			case "short":
				items, err = client.API.ShortCollect(context.Background(), nil)
			case "empty":
				items, err = client.API.EmptyCollect(context.Background(), nil)
			case "offset":
				items, err = client.API.OffsetCollect(context.Background(), nil)
			case "cursor":
				items, err = client.API.CursorCollect(context.Background(), nil)
			case "root":
				items, err = client.API.RootCollect(context.Background(), nil)
			case "nested":
				items, err = client.API.NestedCollect(context.Background(), nil)
			}
			expectedItems := 2
			if name == "short" {
				expectedItems = 3
			}
			if err != nil || len(items) != expectedItems || *calls != 2 || *closed != 2 {
				t.Fatalf("items=%v err=%v calls=%d closed=%d", items, err, *calls, *closed)
			}
		})
	}
}

func TestPaginationLazinessAndInputs(t *testing.T) {
	client, calls, closed := paginationClient(t, func(r *http.Request, n int) (int, string) {
		if r.URL.Query().Get("filter") != "active" || r.URL.Query().Get("extra") != "keep" || r.Header.Get("X-Trace") != "trace" {
			t.Fatalf("lost input: %v", r)
		}
		if r.URL.Query().Get("page") != strconv.Itoa(n-1) || r.URL.Query().Get("limit") != "3" {
			t.Fatal(r.URL)
		}
		return 200, `{"items":[{"id":"a"}],"pages":2}`
	})
	page, limit, filter := int64(99), int64(1), "active"
	params := &PageZeroParams{Page: &page, Limit: &limit, Filter: &filter}
	query := url.Values{"page": []string{"88"}, "limit": []string{"3"}, "extra": []string{"keep"}}
	optionsCalls := 0
	p := client.API.PageZeroPaginator(context.Background(), params, func(o *RequestOptions) {
		optionsCalls++
		o.Query = query
		o.Headers = http.Header{"X-Trace": []string{"trace"}}
	})
	if *calls != 0 {
		t.Fatal("constructor fetched a page")
	}
	if !p.Next() || len(p.Items()) != 1 || *calls != 1 || *closed != 1 {
		t.Fatalf("first page: %v", p.Err())
	}
	if page != 99 || limit != 1 || query.Get("page") != "88" || optionsCalls != 1 {
		t.Fatal("mutated input or reapplied options")
	}
	if !p.Next() || p.Next() || p.Err() != nil || *calls != 2 {
		t.Fatalf("completion: %v", p.Err())
	}
}

func TestPaginationInvalidSizeMakesNoRequests(t *testing.T) {
	client, calls, _ := paginationClient(t, func(*http.Request, int) (int, string) { t.Fatal("unexpected request"); return 200, "" })
	for _, size := range []int64{0, -1, 4} {
		p := client.API.PageZeroPaginator(context.Background(), &PageZeroParams{Limit: &size})
		if p.Next() || p.Err() == nil {
			t.Fatalf("size %d accepted", size)
		}
	}
	for _, value := range []string{"0", "4", "invalid"} {
		p := client.API.PageZeroPaginator(context.Background(), nil, WithRequestOptions(RequestOptions{Query: url.Values{"limit": []string{value}}}))
		if p.Next() || p.Err() == nil {
			t.Fatalf("query size %s accepted", value)
		}
	}
	if *calls != 0 {
		t.Fatal("sent invalid request")
	}
}

func TestPaginationErrorsAndCancellation(t *testing.T) {
	for _, mode := range []string{"http", "decode", "missing", "null", "wrong_type", "empty_more", "metadata", "cursor_loop", "cursor_cycle"} {
		t.Run(mode, func(t *testing.T) {
			client, calls, closed := paginationClient(t, func(_ *http.Request, n int) (int, string) {
				if mode == "cursor_loop" {
					return 200, `{"items":[{"id":"a"}],"next":"same"}`
				}
				if mode == "cursor_cycle" {
					return 200, fmt.Sprintf(`{"items":[{"id":"a"}],"next":"%d"}`, n%2)
				}
				if n == 1 {
					return 200, `{"items":[{"id":"a"}],"pages":2}`
				}
				switch mode {
				case "http":
					return 500, `{"error":"broken"}`
				case "decode":
					return 200, `{`
				case "missing":
					return 200, `{"pages":2}`
				case "null":
					return 200, `{"items":null,"pages":2}`
				case "wrong_type":
					return 200, `{"items":{},"pages":2}`
				case "empty_more":
					return 200, `{"items":[],"pages":3}`
				case "metadata":
					return 200, `{"items":[{"id":"a"}],"pages":null}`
				}
				panic(mode)
			})
			var items []Widget
			var err error
			if strings.HasPrefix(mode, "cursor_") {
				items, err = client.API.CursorCollect(context.Background(), nil)
			} else {
				items, err = client.API.PageZeroCollect(context.Background(), nil)
			}
			if err == nil || items != nil || *calls != *closed {
				t.Fatalf("items=%v err=%v calls=%d closed=%d", items, err, *calls, *closed)
			}
		})
	}
	client, calls, _ := paginationClient(t, func(*http.Request, int) (int, string) { return 200, `{"items":[{"id":"a"}],"pages":2}` })
	ctx, cancel := context.WithCancel(context.Background())
	p := client.API.PageZeroPaginator(ctx, nil)
	if !p.Next() {
		t.Fatal(p.Err())
	}
	cancel()
	if p.Next() || !errors.Is(p.Err(), context.Canceled) || *calls != 1 {
		t.Fatalf("cancellation: %v", p.Err())
	}
}

func TestPaginationErrorHandlerAndEmptyCollection(t *testing.T) {
	sentinel := errors.New("custom error")
	client, _, _ := paginationClient(t, func(*http.Request, int) (int, string) { return 400, `{}` })
	_, err := client.API.PageZeroCollect(context.Background(), nil, WithRequestOptions(RequestOptions{OnError: func(*http.Response) error { return sentinel }}))
	if !errors.Is(err, sentinel) {
		t.Fatalf("lost error handler: %v", err)
	}
	client, _, _ = paginationClient(t, func(*http.Request, int) (int, string) { return 200, `{"items":[],"pages":0}` })
	items, err := client.API.PageZeroCollect(context.Background(), nil)
	if err != nil || !reflect.DeepEqual(items, []Widget{}) {
		t.Fatalf("empty: %v %v", items, err)
	}
}
