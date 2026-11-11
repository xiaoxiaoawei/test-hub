package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"
)

const (
	searchEndpoint     = "https://pro.easyeda.com/api/v2/eda/product/search"
	searchPageSize     = 20
	searchLimit        = 100
	searchMaxBytes     = 8 * 1024 * 1024
	searchTimeout      = 15 * time.Second
	searchCacheTTL     = 2 * time.Minute
	searchCacheEntries = 8
)

type SearchItem struct {
	LCSCId       string `json:"lcscId"`
	Name         string `json:"name"`
	Manufacturer string `json:"manufacturer"`
	Package      string `json:"package"`
	Description  string `json:"description"`
	SourceURL    string `json:"sourceUrl"`
}

type SearchResult struct {
	Query     string       `json:"query"`
	Page      int          `json:"page"`
	PageSize  int          `json:"pageSize"`
	Total     int          `json:"total"`
	HasMore   bool         `json:"hasMore"`
	Items     []SearchItem `json:"items"`
	Source    string       `json:"source"`
	Limit     int          `json:"limit"`
	Truncated bool         `json:"truncated"`
	Message   string       `json:"message"`
}

type searchSnapshot struct {
	result    SearchResult
	createdAt time.Time
	request   *searchRequest
	pending   bool
}

type searchRequest struct{ started time.Time }

// SearchParts performs one explicitly requested lookup against EasyEDA Pro's
// public website search. This is not LCSC's separately approved partner API.
// The public editor currently sends keyword/currPage/pageSize (see its public
// pro-ui.js), but both GET and POST searches ignored currPage in live checks on
// 2026-11-11. Ordering also varied between requests. Fetching at most 100 items
// once and paging that short-lived in-memory snapshot avoids fake pagination.
// No background paging, login, CAPTCHA workaround or search-result persistence
// is performed. Respect https://easyeda.com/page/legal; this is for interactive
// individual lookups, not bulk collection or mirroring.
func (s *Service) SearchParts(ctx context.Context, query string, page int) (SearchResult, error) {
	if !utf8.ValidString(query) || len(query) > 512 {
		return SearchResult{}, errors.New("搜索关键词须为 2–120 个字符")
	}
	for _, r := range query {
		if unicode.IsControl(r) || unicode.Is(unicode.Cf, r) {
			return SearchResult{}, errors.New("搜索关键词不能包含控制字符")
		}
	}
	query = strings.TrimSpace(query)
	if n := utf8.RuneCountInString(query); n < 2 || n > 120 {
		return SearchResult{}, errors.New("搜索关键词须为 2–120 个字符")
	}
	if page < 1 || page > searchLimit/searchPageSize {
		return SearchResult{}, errors.New("搜索页码须为 1–5；仅展示前 100 项，请缩小型号关键词")
	}
	if err := ctx.Err(); err != nil {
		return SearchResult{}, err
	}
	if page > 1 {
		s.searchMu.Lock()
		cached, ok := s.searchSnapshots[query]
		if ok && time.Since(cached.createdAt) >= searchCacheTTL {
			delete(s.searchSnapshots, query)
			ok = false
		}
		s.searchMu.Unlock()
		if !ok || cached.pending {
			return SearchResult{}, errors.New("搜索结果已过期，请重新搜索后再翻页")
		}
		return searchPage(cached.result, page), nil
	}
	// Enter/search on page 1 always refreshes. A failed refresh must not leave
	// stale pages available as though they belonged to the failed new lookup.
	request := s.beginSearch(query)
	published := false
	defer func() {
		if !published {
			s.searchMu.Lock()
			if current, ok := s.searchSnapshots[query]; ok && current.request == request {
				delete(s.searchSnapshots, query)
			}
			s.searchMu.Unlock()
		}
	}()

	params := url.Values{"keyword": {query}, "currPage": {"1"}, "pageSize": {strconv.Itoa(searchLimit)}}
	ctx, cancel := context.WithTimeout(ctx, searchTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, searchEndpoint+"?"+params.Encode(), nil)
	if err != nil {
		return SearchResult{}, err
	}
	req.Header.Set("User-Agent", "PackageStudio/"+Version)
	req.Header.Set("Accept", "application/json")
	// Never follow a login/challenge redirect or send a lookup to another host.
	client := http.Client{}
	if s.client != nil {
		client = *s.client
	}
	client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	resp, err := client.Do(req)
	if err != nil {
		return SearchResult{}, fmt.Errorf("在线搜索失败：%w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return SearchResult{}, searchServiceError(resp.StatusCode, "")
	}
	raw, err := io.ReadAll(io.LimitReader(resp.Body, searchMaxBytes+1))
	if err != nil {
		return SearchResult{}, fmt.Errorf("读取搜索结果失败：%w", err)
	}
	if len(raw) > searchMaxBytes {
		return SearchResult{}, errors.New("搜索结果超过 8 MB 限制，请缩小关键词")
	}
	if strings.Contains(strings.ToLower(resp.Header.Get("Content-Type")), "text/html") || strings.HasPrefix(strings.TrimSpace(string(raw)), "<") {
		return SearchResult{}, errors.New("搜索服务返回网页，可能需要登录或人机验证；请稍后重试")
	}
	var payload struct {
		Code    *int   `json:"code"`
		Success *bool  `json:"success"`
		Msg     string `json:"msg"`
		Message string `json:"message"`
		Result  *struct {
			Total    *int              `json:"total"`
			Products []json.RawMessage `json:"productList"`
		} `json:"result"`
	}
	if json.Unmarshal(raw, &payload) != nil || payload.Code == nil {
		return SearchResult{}, errors.New("搜索服务返回了无法识别的数据")
	}
	message := payload.Msg
	if message == "" {
		message = payload.Message
	}
	if *payload.Code != 0 || payload.Success != nil && !*payload.Success {
		return SearchResult{}, searchServiceError(*payload.Code, message)
	}
	if payload.Result == nil || payload.Result.Total == nil || *payload.Result.Total < 0 || *payload.Result.Total > 100_000_000 || payload.Result.Products == nil {
		return SearchResult{}, errors.New("搜索服务返回了不完整的结果")
	}
	products := payload.Result.Products
	result := SearchResult{Query: query, Page: page, PageSize: searchPageSize, Total: *payload.Result.Total,
		Items: []SearchItem{}, Source: "EasyEDA Pro 公开器件搜索", Limit: searchLimit}
	if result.Total < len(products) {
		return SearchResult{}, errors.New("搜索服务返回了不一致的结果数量")
	}
	if result.Total > searchLimit {
		result.Truncated = true
		result.Message = "仅展示前 100 项，请缩小型号关键词以获得更准确的结果。"
	}
	if len(products) < searchLimit && len(products) < result.Total {
		result.Truncated = true
		result.Message = "上游服务限制了返回数量，仅展示本次已返回的结果；请缩小型号关键词。"
	}
	seen := map[string]bool{}
	for _, rawProduct := range products[:min(len(products), searchLimit)] {
		item, ok := searchProduct(rawProduct)
		if !ok || seen[item.LCSCId] {
			continue
		}
		seen[item.LCSCId] = true
		result.Items = append(result.Items, item)
	}
	if len(products) > 0 && len(result.Items) == 0 {
		return SearchResult{}, errors.New("搜索服务未返回有效的立创编号，请更换关键词")
	}
	if len(result.Items) < min(len(products), searchLimit) {
		result.Message = strings.TrimSpace(result.Message + " 部分结果缺少有效立创编号或重复，已略过。")
	}
	if err := ctx.Err(); err != nil {
		return SearchResult{}, err
	}
	s.searchMu.Lock()
	if current, ok := s.searchSnapshots[query]; !ok || current.request != request {
		s.searchMu.Unlock()
		return SearchResult{}, context.Canceled
	}
	s.searchSnapshots[query] = searchSnapshot{result: result, createdAt: time.Now(), request: request}
	published = true
	s.searchMu.Unlock()
	return searchPage(result, page), nil
}

// Reserve a generation before starting the network request. A late response
// from an older refresh cannot replace (or delete on failure) a newer snapshot.
func (s *Service) beginSearch(query string) *searchRequest {
	s.searchMu.Lock()
	defer s.searchMu.Unlock()
	if s.searchSnapshots == nil {
		s.searchSnapshots = make(map[string]searchSnapshot)
	}
	now := time.Now()
	delete(s.searchSnapshots, query)
	for key, snapshot := range s.searchSnapshots {
		if now.Sub(snapshot.createdAt) >= searchCacheTTL {
			delete(s.searchSnapshots, key)
		}
	}
	if len(s.searchSnapshots) >= searchCacheEntries {
		oldestKey := ""
		var oldest time.Time
		for key, snapshot := range s.searchSnapshots {
			if oldestKey == "" || snapshot.createdAt.Before(oldest) {
				oldestKey, oldest = key, snapshot.createdAt
			}
		}
		delete(s.searchSnapshots, oldestKey)
	}
	request := &searchRequest{started: now}
	s.searchSnapshots[query] = searchSnapshot{createdAt: now, request: request, pending: true}
	return request
}

func searchPage(snapshot SearchResult, page int) SearchResult {
	result := snapshot
	result.Page = page
	start := min((page-1)*searchPageSize, len(snapshot.Items))
	end := min(page*searchPageSize, len(snapshot.Items))
	result.Items = make([]SearchItem, end-start)
	copy(result.Items, snapshot.Items[start:end])
	result.HasMore = end < len(snapshot.Items) && page < searchLimit/searchPageSize
	return result
}

func searchServiceError(code int, message string) error {
	lower := strings.ToLower(message)
	switch {
	case code == http.StatusTooManyRequests || strings.Contains(lower, "rate limit") || strings.Contains(lower, "too many") || strings.Contains(lower, "频繁") || strings.Contains(lower, "限流"):
		return errors.New("搜索请求过于频繁，请稍后重试")
	case code == http.StatusUnauthorized || code == http.StatusForbidden || code >= 300 && code < 400 || strings.Contains(lower, "login") || strings.Contains(lower, "sign in") || strings.Contains(lower, "captcha") || strings.Contains(lower, "登录") || strings.Contains(lower, "验证"):
		return errors.New("搜索服务要求登录、授权或人机验证，当前无法访问")
	default:
		return fmt.Errorf("搜索服务暂不可用（代码 %d），请稍后重试", code)
	}
}

func searchProduct(raw json.RawMessage) (SearchItem, bool) {
	var p struct {
		Number       string          `json:"number"`
		MPN          string          `json:"mpn"`
		Manufacturer string          `json:"manufacturer"`
		Package      string          `json:"package"`
		Description  string          `json:"description"`
		URL          string          `json:"url"`
		DeviceInfo   json.RawMessage `json:"device_info"`
	}
	if json.Unmarshal(raw, &p) != nil {
		return SearchItem{}, false
	}
	id := strings.ToUpper(strings.TrimSpace(p.Number))
	if !lcscPattern.MatchString(id) {
		return SearchItem{}, false
	}
	var device struct {
		Attributes map[string]string `json:"attributes"`
	}
	// Some genuine results have device_info:[] when no Pro device is attached.
	// The product remains discoverable; Standard geometry is fetched separately.
	_ = json.Unmarshal(p.DeviceInfo, &device)
	name := searchText(p.MPN, 240)
	if name == "" {
		name = searchText(device.Attributes["Manufacturer Part"], 240)
	}
	if name == "" {
		name = id
	}
	description := searchText(p.Description, 1200)
	if description == "" {
		description = searchText(device.Attributes["LCSC Part Name"], 1200)
	}
	return SearchItem{LCSCId: id, Name: name, Manufacturer: searchText(p.Manufacturer, 240), Package: searchText(p.Package, 240), Description: description, SourceURL: searchSourceURL(p.URL)}, true
}

func searchText(text string, limit int) string {
	text = strings.Map(func(r rune) rune {
		if unicode.IsControl(r) || unicode.Is(unicode.Cf, r) {
			return ' '
		}
		return r
	}, text)
	text = strings.TrimSpace(text)
	runes := []rune(text)
	if len(runes) > limit {
		text = string(runes[:limit])
	}
	return text
}

func searchSourceURL(value string) string {
	if len(value) > 2048 || strings.IndexFunc(value, func(r rune) bool { return unicode.IsControl(r) || unicode.Is(unicode.Cf, r) }) >= 0 {
		return ""
	}
	u, err := url.Parse(value)
	if err != nil || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return ""
	}
	if u.IsAbs() {
		if u.Scheme != "https" || u.Host != "www.lcsc.com" && u.Host != "lcsc.com" {
			return ""
		}
	} else {
		if u.Host != "" || !strings.HasPrefix(u.Path, "/product-detail/") {
			return ""
		}
		u.Scheme, u.Host = "https", "www.lcsc.com"
	}
	if !strings.HasPrefix(u.Path, "/product-detail/") || strings.Contains(u.Path, "..") {
		return ""
	}
	return u.String()
}
