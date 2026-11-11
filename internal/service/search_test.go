package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// Reduced public response, captured without credentials on 2026-11-11 from
// https://pro.easyeda.com/api/v2/eda/product/search?keyword=STM32F030C8T6&currPage=1&pageSize=20.
// Prices, inventory, large geometry, user metadata and unrelated fields omitted.
const stm32SearchFixture = `{"code":0,"success":true,"msg":null,"result":{"total":2,"productList":[
 {"mpn":"STM32F030C8T6","number":"C23922","package":"LQFP-48(7x7)","manufacturer":"ST","url":"/product-detail/Microcontrollers--MCU-MPU-SOC-_ST-STM32F030C8T6_C23922.html","device_info":{"attributes":{"LCSC Part Name":"主流ARM Cortex-M0超值系列MCU，具有64 KB Flash和48 MHz CPU"}}},
 {"mpn":"STM32F030C8T6TR","number":"C40053","package":"LQFP-48(7x7)","manufacturer":"ST","url":"/product-detail/Microcontrollers--MCU-MPU-SOC-_ST-STM32F030C8T6TR_C40053.html"}
]}}`

func mockSearchService(t *testing.T, fn func(*http.Request) (*http.Response, error)) *Service {
	t.Helper()
	return &Service{client: &http.Client{Transport: transportFunc(fn)}}
}

func searchResponse(status int, raw string) *http.Response {
	return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(raw))}
}

func TestSearchRealModelFixtureKeepsEmbeddedCAndMapsSource(t *testing.T) {
	s := mockSearchService(t, func(r *http.Request) (*http.Response, error) {
		if r.Method != "GET" || r.URL.Scheme != "https" || r.URL.Host != "pro.easyeda.com" || r.URL.Path != "/api/v2/eda/product/search" {
			t.Fatalf("wrong public search endpoint: %s %s", r.Method, r.URL)
		}
		q := r.URL.Query()
		if q.Get("keyword") != "STM32F030C8T6" || q.Get("currPage") != "1" || q.Get("pageSize") != "100" || len(q) != 3 {
			t.Fatalf("model or paging query was changed: %s", r.URL.RawQuery)
		}
		deadline, ok := r.Context().Deadline()
		if !ok || time.Until(deadline) > searchTimeout || time.Until(deadline) < searchTimeout-time.Second {
			t.Fatalf("missing bounded search timeout: %v %v", deadline, ok)
		}
		if r.Header.Get("Accept") != "application/json" || r.Header.Get("User-Agent") != "PackageStudio/"+Version || r.Header.Get("Cookie") != "" || r.Header.Get("Authorization") != "" {
			t.Fatalf("unexpected search headers: %v", r.Header)
		}
		return searchResponse(200, stm32SearchFixture), nil
	})
	r, err := s.SearchParts(context.Background(), " STM32F030C8T6 ", 1)
	if err != nil {
		t.Fatal(err)
	}
	if r.Query != "STM32F030C8T6" || r.Page != 1 || r.PageSize != 20 || r.Total != 2 || r.HasMore || r.Limit != 100 || r.Truncated || len(r.Items) != 2 {
		t.Fatalf("wrong search metadata: %+v", r)
	}
	a, b := r.Items[0], r.Items[1]
	if a.LCSCId != "C23922" || a.Name != "STM32F030C8T6" || a.Package != "LQFP-48(7x7)" || a.Manufacturer != "ST" || !strings.Contains(a.Description, "Cortex-M0") || b.LCSCId != "C40053" {
		t.Fatalf("lost real public fields: %+v", r.Items)
	}
	if a.SourceURL != "https://www.lcsc.com/product-detail/Microcontrollers--MCU-MPU-SOC-_ST-STM32F030C8T6_C23922.html" {
		t.Fatalf("lost upstream source path: %s", a.SourceURL)
	}
	raw, _ := json.Marshal(r)
	if !strings.Contains(string(raw), `"lcscId":"C23922"`) || strings.Contains(string(raw), `"LCSCId"`) {
		t.Fatalf("frontend field contract changed: %s", raw)
	}
}

func TestSearchEscapesModelQueryWithoutAddingURLParameters(t *testing.T) {
	query := "MP1584EN-LF-Z/3.3V + µC &pageSize=999"
	s := mockSearchService(t, func(r *http.Request) (*http.Response, error) {
		if r.URL.Query().Get("keyword") != query || r.URL.Query().Get("pageSize") != "100" || len(r.URL.Query()) != 3 {
			t.Fatalf("unsafe keyword encoding: %s", r.URL)
		}
		return searchResponse(200, `{"code":0,"result":{"total":0,"productList":[]}}`), nil
	})
	r, err := s.SearchParts(context.Background(), query, 1)
	if err != nil || r.Items == nil || len(r.Items) != 0 || r.Total != 0 || r.HasMore || r.Truncated {
		t.Fatalf("empty search must remain a valid empty array: %+v, %v", r, err)
	}
}

func TestSearchRejectsInvalidInputBeforeNetwork(t *testing.T) {
	s := mockSearchService(t, func(*http.Request) (*http.Response, error) { t.Fatal("invalid query reached network"); return nil, nil })
	for _, query := range []string{"", " ", "x", strings.Repeat("型", 121), strings.Repeat("x", 513), "LM358\n", "\tLM358", "LM\x00358", "LM\u200b358", "LM\u202e358", string([]byte{0xff, 0xfe})} {
		if _, err := s.SearchParts(context.Background(), query, 1); err == nil {
			t.Errorf("accepted bad query %q", query)
		}
	}
	for _, page := range []int{-1, 0, 6, 1_000_000} {
		if _, err := s.SearchParts(context.Background(), "LM358", page); err == nil {
			t.Errorf("accepted page %d", page)
		}
	}
}

func syntheticSearchPrefix(count, total int) string {
	items := make([]map[string]string, count)
	for i := range items {
		items[i] = map[string]string{"number": fmt.Sprintf("C%d", 10000+i), "mpn": "synthetic test result"}
	}
	raw, _ := json.Marshal(map[string]any{"code": 0, "success": true, "result": map[string]any{"total": total, "productList": items}})
	return string(raw)
}

func TestSearchSnapshotPaginationHasDistinctPagesAndHardLimit(t *testing.T) {
	for _, page := range []int{1, 2, 5} {
		t.Run(fmt.Sprint(page), func(t *testing.T) {
			calls := 0
			s := mockSearchService(t, func(r *http.Request) (*http.Response, error) {
				calls++
				if r.URL.Query().Get("currPage") != "1" || r.URL.Query().Get("pageSize") != "100" {
					t.Fatalf("upstream pagination bug reintroduced: %s", r.URL)
				}
				return searchResponse(200, syntheticSearchPrefix(100, 206)), nil
			})
			if page > 1 {
				if _, err := s.SearchParts(context.Background(), "LM358", 1); err != nil {
					t.Fatal(err)
				}
			}
			r, err := s.SearchParts(context.Background(), "LM358", page)
			if err != nil {
				t.Fatal(err)
			}
			if calls != 1 || len(r.Items) != 20 || r.Items[0].LCSCId != fmt.Sprintf("C%d", 10000+(page-1)*20) || r.Items[19].LCSCId != fmt.Sprintf("C%d", 10000+page*20-1) {
				t.Fatalf("wrong snapshot slice or automatic requests: calls=%d result=%+v", calls, r)
			}
			if r.Total != 206 || r.Limit != 100 || !r.Truncated || r.Message == "" || r.HasMore != (page < 5) {
				t.Fatalf("wrong bounded paging metadata: %+v", r)
			}
		})
	}
}

func TestSearchStopsWhenUpstreamTruncatesOrTotalIsReached(t *testing.T) {
	for _, tc := range []struct {
		name               string
		count, total, page int
		truncated          bool
		wantItems          int
	}{
		{"upstream stops before page", 20, 206, 2, true, 0},
		{"upstream partial page", 25, 206, 2, true, 5},
		{"genuine final page", 23, 23, 2, false, 3},
		{"empty past final page", 2, 2, 2, false, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := mockSearchService(t, func(*http.Request) (*http.Response, error) {
				return searchResponse(200, syntheticSearchPrefix(tc.count, tc.total)), nil
			})
			if tc.page > 1 {
				if _, err := s.SearchParts(context.Background(), "LM358", 1); err != nil {
					t.Fatal(err)
				}
			}
			r, err := s.SearchParts(context.Background(), "LM358", tc.page)
			if err != nil || r.HasMore || r.Truncated != tc.truncated || len(r.Items) != tc.wantItems {
				t.Fatalf("bad truncation handling: %+v, %v", r, err)
			}
			if tc.truncated && r.Message == "" {
				t.Fatal("upstream truncation was silent")
			}
		})
	}
}

func TestSearchSnapshotStaysStableWhenUpstreamOrderChangesAndRefreshIsExplicit(t *testing.T) {
	calls := 0
	s := mockSearchService(t, func(*http.Request) (*http.Response, error) {
		calls++
		raw := syntheticSearchPrefix(100, 206)
		if calls > 1 {
			var payload map[string]any
			_ = json.Unmarshal([]byte(raw), &payload)
			items := payload["result"].(map[string]any)["productList"].([]any)
			for i, j := 0, len(items)-1; i < j; i, j = i+1, j-1 {
				items[i], items[j] = items[j], items[i]
			}
			b, _ := json.Marshal(payload)
			raw = string(b)
		}
		return searchResponse(200, raw), nil
	})
	a, err := s.SearchParts(context.Background(), "LM358", 1)
	if err != nil {
		t.Fatal(err)
	}
	a.Items[0].LCSCId = "caller-mutation"
	b, err := s.SearchParts(context.Background(), "LM358", 2)
	if err != nil || calls != 1 || b.Items[0].LCSCId != "C10020" {
		t.Fatalf("snapshot changed while paging: %+v %v calls=%d", b, err, calls)
	}
	if s.searchSnapshots["LM358"].result.Items[0].LCSCId != "C10000" {
		t.Fatal("caller can mutate cached snapshot")
	}
	a, err = s.SearchParts(context.Background(), "LM358", 1)
	if err != nil || calls != 2 || a.Items[0].LCSCId != "C10099" {
		t.Fatalf("page 1 did not refresh: %+v %v calls=%d", a, err, calls)
	}
	b, err = s.SearchParts(context.Background(), "LM358", 2)
	if err != nil || calls != 2 || b.Items[0].LCSCId != "C10079" {
		t.Fatalf("refreshed snapshot page mismatch: %+v %v calls=%d", b, err, calls)
	}
}

func TestSearchSnapshotExpiryCacheBoundAndFailedRefresh(t *testing.T) {
	calls := 0
	s := mockSearchService(t, func(*http.Request) (*http.Response, error) {
		calls++
		return searchResponse(200, syntheticSearchPrefix(100, 206)), nil
	})
	if _, err := s.SearchParts(context.Background(), "LM358", 2); err == nil || calls != 0 {
		t.Fatal("missing snapshot must ask to search first")
	}
	if _, err := s.SearchParts(context.Background(), "LM358", 1); err != nil {
		t.Fatal(err)
	}
	cached := s.searchSnapshots["LM358"]
	cached.createdAt = time.Now().Add(-searchCacheTTL - time.Second)
	s.searchSnapshots["LM358"] = cached
	if _, err := s.SearchParts(context.Background(), "LM358", 2); err == nil || !strings.Contains(err.Error(), "过期") || calls != 1 {
		t.Fatal("expired snapshot silently requested different results")
	}
	for i := 0; i < searchCacheEntries+1; i++ {
		if _, err := s.SearchParts(context.Background(), fmt.Sprintf("model-%d", i), 1); err != nil {
			t.Fatal(err)
		}
	}
	if len(s.searchSnapshots) != searchCacheEntries {
		t.Fatalf("unbounded cache: %d", len(s.searchSnapshots))
	}
	if _, ok := s.searchSnapshots["model-0"]; ok {
		t.Fatal("oldest snapshot was not evicted")
	}
	s.client.Transport = transportFunc(func(*http.Request) (*http.Response, error) { return nil, errors.New("offline") })
	if _, err := s.SearchParts(context.Background(), "model-8", 1); err == nil {
		t.Fatal("expected failed refresh")
	}
	if _, err := s.SearchParts(context.Background(), "model-8", 2); err == nil || !strings.Contains(err.Error(), "过期") {
		t.Fatal("failed refresh left stale pages available")
	}
}

func TestSearchLateOlderRefreshCannotReplaceOrDeleteNewSnapshot(t *testing.T) {
	for _, olderFails := range []bool{false, true} {
		t.Run(fmt.Sprintf("olderFails=%v", olderFails), func(t *testing.T) {
			started, release := make(chan struct{}), make(chan struct{})
			var calls atomic.Int32
			s := mockSearchService(t, func(*http.Request) (*http.Response, error) {
				if calls.Add(1) == 1 {
					close(started)
					<-release
					if olderFails {
						return nil, errors.New("older request failed")
					}
					return searchResponse(200, syntheticSearchPrefix(100, 206)), nil
				}
				return searchResponse(200, strings.ReplaceAll(syntheticSearchPrefix(100, 206), "C100", "C200")), nil
			})
			oldDone := make(chan error, 1)
			go func() { _, err := s.SearchParts(context.Background(), "LM358", 1); oldDone <- err }()
			<-started
			if _, err := s.SearchParts(context.Background(), "LM358", 2); err == nil {
				t.Fatal("pending snapshot returned zero-value results")
			}
			newer, err := s.SearchParts(context.Background(), "LM358", 1)
			if err != nil || newer.Items[0].LCSCId != "C20000" {
				t.Fatalf("newer refresh failed: %+v %v", newer, err)
			}
			close(release)
			if err := <-oldDone; err == nil {
				t.Fatal("superseded response should not be published")
			}
			page2, err := s.SearchParts(context.Background(), "LM358", 2)
			if err != nil || len(page2.Items) != 20 || page2.Items[0].LCSCId != "C20020" || calls.Load() != 2 {
				t.Fatalf("old response replaced/deleted new snapshot: %+v %v", page2, err)
			}
		})
	}
}

func TestSearchFailedNewRefreshCannotResurrectOldResponse(t *testing.T) {
	started, release := make(chan struct{}), make(chan struct{})
	var calls atomic.Int32
	s := mockSearchService(t, func(*http.Request) (*http.Response, error) {
		if calls.Add(1) == 1 {
			close(started)
			<-release
			return searchResponse(200, syntheticSearchPrefix(100, 206)), nil
		}
		return nil, errors.New("newer request failed")
	})
	oldDone := make(chan error, 1)
	go func() { _, err := s.SearchParts(context.Background(), "LM358", 1); oldDone <- err }()
	<-started
	if _, err := s.SearchParts(context.Background(), "LM358", 1); err == nil {
		t.Fatal("expected newer request failure")
	}
	close(release)
	if err := <-oldDone; !errors.Is(err, context.Canceled) {
		t.Fatalf("older success survived failed newer refresh: %v", err)
	}
	if _, err := s.SearchParts(context.Background(), "LM358", 2); err == nil {
		t.Fatal("failed refresh resurrected old pages")
	}
}

func TestSearchLivePublicEndpoint(t *testing.T) {
	if os.Getenv("PACKAGESTUDIO_LIVE_SEARCH") != "1" {
		t.Skip("opt-in public endpoint smoke test")
	}
	s := &Service{client: &http.Client{Timeout: 20 * time.Second}}
	for _, query := range []string{"STM32F030C8T6", "MP1584EN-LF-Z", "LM358", "PackageStudio-NoSuchPart-9F6C82"} {
		a, err := s.SearchParts(context.Background(), query, 1)
		if err != nil {
			t.Fatalf("%s: %v", query, err)
		}
		firstIDs := []string{}
		for _, item := range a.Items[:min(3, len(a.Items))] {
			firstIDs = append(firstIDs, item.LCSCId)
		}
		t.Logf("%s: total=%d page1=%d first=%v hasMore=%v truncated=%v", query, a.Total, len(a.Items), firstIDs, a.HasMore, a.Truncated)
		if strings.HasPrefix(query, "PackageStudio-") {
			if a.Total != 0 || len(a.Items) != 0 || a.HasMore {
				t.Fatal("no-match query did not return genuine empty result")
			}
			continue
		}
		if a.Total < 1 || len(a.Items) < 1 {
			t.Fatalf("known model has no public result: %s", query)
		}
		if query == "STM32F030C8T6" {
			found := false
			for _, item := range a.Items {
				found = found || item.LCSCId == "C23922"
			}
			if !found {
				t.Fatalf("unexpected STM32 match: %+v", a.Items)
			}
		}
		if query == "LM358" {
			b, err := s.SearchParts(context.Background(), query, 2)
			if err != nil || len(b.Items) != 20 {
				t.Fatalf("LM358 page2: %+v %v", b, err)
			}
			seen := map[string]bool{}
			for _, item := range a.Items {
				seen[item.LCSCId] = true
			}
			for _, item := range b.Items {
				if seen[item.LCSCId] {
					t.Fatalf("same-snapshot pages overlap at %s", item.LCSCId)
				}
			}
			t.Logf("LM358 page2=%d first=%s (same snapshot, distinct from all first-page IDs)", len(b.Items), b.Items[0].LCSCId)
		}
	}
}

func TestSearchFiltersMissingInvalidDuplicateIDsAndUnsafeLinks(t *testing.T) {
	raw := `{"code":0,"result":{"total":7,"productList":[
	{"number":"C23922","mpn":"STM32","device_info":[],"url":"https://evil.example/product-detail/x"},
	{"number":" c23922 ","mpn":"duplicate"},
	{"mpn":"no number"},
	{"number":"C23922?x=1"},
	{"number":123},
	{"number":"C40053","device_info":{"attributes":{"Manufacturer Part":"STM32F030C8T6TR","LCSC Part Name":"public\npart"}},"url":"//evil.example/product-detail/x"},
	{"number":"C25804","mpn":"R\u202e_0603","url":"https://www.lcsc.com/product-detail/test_C25804.html"}
	]}}`
	s := mockSearchService(t, func(*http.Request) (*http.Response, error) { return searchResponse(200, raw), nil })
	r, err := s.SearchParts(context.Background(), "STM32", 1)
	if err != nil || len(r.Items) != 3 {
		t.Fatalf("failed to filter unsafe records: %+v %v", r, err)
	}
	if r.Items[0].SourceURL != "" || r.Items[1].SourceURL != "" || r.Items[1].Name != "STM32F030C8T6TR" || r.Items[1].Description != "public part" || r.Items[2].Name != "R _0603" || r.Items[2].SourceURL == "" {
		t.Fatalf("unsafe text/source or lost fallbacks: %+v", r.Items)
	}
	for _, rawURL := range []string{"javascript:alert(1)", "http://www.lcsc.com/product-detail/x", "https://user@www.lcsc.com/product-detail/x", "https://www.lcsc.com:443/product-detail/x", "/product-detail/../x", "/product-detail/%2e%2e/x", "/product-detail/x?next=evil", "/product-detail/x#frag", "/other", "/product-detail/x\n"} {
		if got := searchSourceURL(rawURL); got != "" {
			t.Errorf("accepted unsafe source %q => %q", rawURL, got)
		}
	}
}

func TestSearchReportsHTTPBusinessLoginAndMalformedResponses(t *testing.T) {
	for _, tc := range []struct {
		name           string
		status         int
		body, contains string
	}{
		{"http throttling", 429, `{}`, "频繁"},
		{"http login", 401, `{}`, "登录"},
		{"http forbidden", 403, `{}`, "验证"},
		{"http redirect", 302, `{}`, "登录"},
		{"http unavailable", 503, `{}`, "503"},
		{"business throttling", 200, `{"code":429,"msg":"too many requests"}`, "频繁"},
		{"business login", 200, `{"code":-1,"msg":"Please login"}`, "登录"},
		{"business challenge", 200, `{"code":-1,"message":"captcha required"}`, "验证"},
		{"business fail", 200, `{"code":12,"msg":"temporary failure"}`, "12"},
		{"false success", 200, `{"code":0,"success":false,"msg":"temporary failure"}`, "不可用"},
		{"html challenge", 200, `<html>verify</html>`, "验证"},
		{"malformed", 200, `not JSON`, "无法识别"},
		{"missing code", 200, `{"result":{"total":0,"productList":[]}}`, "无法识别"},
		{"missing result", 200, `{"code":0}`, "不完整"},
		{"negative total", 200, `{"code":0,"result":{"total":-1,"productList":[]}}`, "不完整"},
		{"null products", 200, `{"code":0,"result":{"total":0,"productList":null}}`, "不完整"},
		{"inconsistent total", 200, `{"code":0,"result":{"total":0,"productList":[{"number":"C23922"}]}}`, "不一致"},
		{"no valid ids", 200, `{"code":0,"result":{"total":1,"productList":[{"mpn":"unknown"}]}}`, "有效"},
		{"oversized", 200, strings.Repeat(" ", searchMaxBytes+1), "8 MB"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := mockSearchService(t, func(*http.Request) (*http.Response, error) { return searchResponse(tc.status, tc.body), nil })
			_, err := s.SearchParts(context.Background(), "LM358", 1)
			if err == nil || !strings.Contains(err.Error(), tc.contains) {
				t.Fatalf("wrong failure: %v", err)
			}
		})
	}
}

func TestSearchPreservesCancellationAndNetworkFailure(t *testing.T) {
	for _, failure := range []error{context.Canceled, context.DeadlineExceeded, errors.New("offline")} {
		s := mockSearchService(t, func(r *http.Request) (*http.Response, error) { return nil, failure })
		_, err := s.SearchParts(context.Background(), "LM358", 1)
		if !errors.Is(err, failure) {
			t.Fatalf("lost network cause: %v", err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	s := mockSearchService(t, func(r *http.Request) (*http.Response, error) {
		if r.Context().Err() != context.Canceled {
			t.Fatal("caller cancellation lost")
		}
		return nil, r.Context().Err()
	})
	if _, err := s.SearchParts(ctx, "LM358", 1); !errors.Is(err, context.Canceled) {
		t.Fatalf("caller cancellation lost: %v", err)
	}
}

func TestSearchDoesNotFollowChallengeRedirect(t *testing.T) {
	hits := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { hits++; http.Redirect(w, r, "/login", http.StatusFound) }))
	defer server.Close()
	// Re-route only this test's initial official URL into the local server. The
	// real client's redirect policy must stop before a second HTTP request.
	transport := http.DefaultTransport.(*http.Transport).Clone()
	defer transport.CloseIdleConnections()
	s := mockSearchService(t, func(r *http.Request) (*http.Response, error) {
		clone := r.Clone(r.Context())
		u := *r.URL
		clone.URL = &u
		clone.URL.Scheme = "http"
		clone.URL.Host = strings.TrimPrefix(server.URL, "http://")
		return transport.RoundTrip(clone)
	})
	if _, err := s.SearchParts(context.Background(), "LM358", 1); err == nil || hits != 1 {
		t.Fatalf("challenge redirect followed: hits=%d err=%v", hits, err)
	}
}
