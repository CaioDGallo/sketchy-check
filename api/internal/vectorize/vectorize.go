// Package vectorize parses the /fraud-score JSON payload and produces the
// 14-dimension feature vector. It deliberately avoids encoding/json — that
// path's reflection and allocation cost dominates a sub-millisecond budget.
//
// The parser is intentionally permissive: it scans for known keys within
// pre-located object/array bounds and tolerates whitespace. It does NOT
// attempt to validate the JSON's overall well-formedness; malformed input
// returns an error and the server responds 400.
package vectorize

import (
	"bytes"
	"errors"
	"strconv"
	"unsafe"

	"github.com/rinha2026/sketchy/api/internal/mcc"
)

// Constants from resources/normalization.json. These never change during the
// edition, so we compile them in.
const (
	maxAmount             = 10000.0
	maxInstallments       = 12.0
	amountVsAvgRatio      = 10.0
	maxMinutes            = 1440.0
	maxKM                 = 1000.0
	maxTxCount24h         = 20.0
	maxMerchantAvgAmount  = 10000.0
)

// ErrParse is returned when the JSON payload is missing required fields or
// has malformed values. Callers map this to HTTP 400.
var ErrParse = errors.New("vectorize: malformed payload")

// Build parses body and writes 14 normalized features into q.
//
// Index meaning (from DETECTION_RULES.md):
//
//	0  amount                clamp(amount / 10000)
//	1  installments          clamp(installments / 12)
//	2  amount_vs_avg         clamp((amount / customer.avg_amount) / 10)
//	3  hour_of_day           hour / 23 (UTC)
//	4  day_of_week           weekday / 6 (Mon=0..Sun=6)
//	5  minutes_since_last_tx clamp(min / 1440) OR -1 if last_transaction is null
//	6  km_from_last_tx       clamp(km_from_current / 1000) OR -1 if null
//	7  km_from_home          clamp(km_from_home / 1000)
//	8  tx_count_24h          clamp(tx_count_24h / 20)
//	9  is_online             1.0 / 0.0
//	10 card_present          1.0 / 0.0
//	11 unknown_merchant      1.0 if merchant.id NOT in customer.known_merchants
//	12 mcc_risk              from mcc package
//	13 merchant_avg_amount   clamp(merchant.avg_amount / 10000)
func Build(body []byte, q *[14]float32) error {
	txStart, txEnd, ok := objectRange(body, 0, len(body), keyTransaction)
	if !ok {
		return ErrParse
	}
	custStart, custEnd, ok := objectRange(body, 0, len(body), keyCustomer)
	if !ok {
		return ErrParse
	}
	merchStart, merchEnd, ok := objectRange(body, 0, len(body), keyMerchant)
	if !ok {
		return ErrParse
	}
	termStart, termEnd, ok := objectRange(body, 0, len(body), keyTerminal)
	if !ok {
		return ErrParse
	}

	amount, ok := jsonNumber(body, txStart, txEnd, keyAmount)
	if !ok {
		return ErrParse
	}
	installments, ok := jsonNumber(body, txStart, txEnd, keyInstallments)
	if !ok {
		return ErrParse
	}
	requestedAt, ok := jsonString(body, txStart, txEnd, keyRequestedAt)
	if !ok || len(requestedAt) < 19 {
		return ErrParse
	}

	customerAvg, ok := jsonNumber(body, custStart, custEnd, keyAvgAmount)
	if !ok {
		return ErrParse
	}
	tx24h, ok := jsonNumber(body, custStart, custEnd, keyTxCount24h)
	if !ok {
		return ErrParse
	}

	merchantID, ok := jsonString(body, merchStart, merchEnd, keyID)
	if !ok {
		return ErrParse
	}
	mccBytes, ok := jsonString(body, merchStart, merchEnd, keyMCC)
	if !ok {
		return ErrParse
	}
	merchantAvg, ok := jsonNumber(body, merchStart, merchEnd, keyAvgAmount)
	if !ok {
		return ErrParse
	}

	isOnline, ok := jsonBool(body, termStart, termEnd, keyIsOnline)
	if !ok {
		return ErrParse
	}
	cardPresent, ok := jsonBool(body, termStart, termEnd, keyCardPresent)
	if !ok {
		return ErrParse
	}
	kmFromHome, ok := jsonNumber(body, termStart, termEnd, keyKMFromHome)
	if !ok {
		return ErrParse
	}

	// last_transaction is optional and may be null.
	minutesSinceLast := float32(-1)
	kmFromLast := float32(-1)
	if ltStart, ltEnd, isObj := objectRange(body, 0, len(body), keyLastTransaction); isObj {
		lastTS, ok := jsonString(body, ltStart, ltEnd, keyTimestamp)
		if !ok || len(lastTS) < 19 {
			return ErrParse
		}
		kmFromCurrent, ok := jsonNumber(body, ltStart, ltEnd, keyKMFromCurrent)
		if !ok {
			return ErrParse
		}
		mins := minutesBetween(requestedAt, lastTS)
		minutesSinceLast = clamp01(float32(mins) / maxMinutes)
		kmFromLast = clamp01(kmFromCurrent / maxKM)
	}

	// unknown_merchant: 1.0 if merchant.id NOT present in customer.known_merchants.
	known := arrayContainsString(body, custStart, custEnd, keyKnownMerchants, merchantID)
	unknownMerchant := float32(1)
	if known {
		unknownMerchant = 0
	}

	amountVsAvg := float32(1)
	if customerAvg > 0 {
		amountVsAvg = clamp01((amount / customerAvg) / amountVsAvgRatio)
	}

	q[0] = clamp01(amount / maxAmount)
	q[1] = clamp01(installments / maxInstallments)
	q[2] = amountVsAvg
	q[3] = clamp01(float32(isoHourUTC(requestedAt)) / 23.0)
	q[4] = clamp01(float32(weekdayMondayZero(requestedAt)) / 6.0)
	q[5] = minutesSinceLast
	q[6] = kmFromLast
	q[7] = clamp01(kmFromHome / maxKM)
	q[8] = clamp01(tx24h / maxTxCount24h)
	if isOnline {
		q[9] = 1
	} else {
		q[9] = 0
	}
	if cardPresent {
		q[10] = 1
	} else {
		q[10] = 0
	}
	q[11] = unknownMerchant
	q[12] = mcc.Get(mccBytes)
	q[13] = clamp01(merchantAvg / maxMerchantAvgAmount)
	return nil
}

// Pre-quoted key bytes. Allocating these once saves both the alloc and the
// implicit "key" → []byte conversion on every request.
var (
	keyTransaction     = []byte(`"transaction"`)
	keyCustomer        = []byte(`"customer"`)
	keyMerchant        = []byte(`"merchant"`)
	keyTerminal        = []byte(`"terminal"`)
	keyLastTransaction = []byte(`"last_transaction"`)
	keyAmount          = []byte(`"amount"`)
	keyInstallments    = []byte(`"installments"`)
	keyRequestedAt     = []byte(`"requested_at"`)
	keyAvgAmount       = []byte(`"avg_amount"`)
	keyTxCount24h      = []byte(`"tx_count_24h"`)
	keyKnownMerchants  = []byte(`"known_merchants"`)
	keyID              = []byte(`"id"`)
	keyMCC             = []byte(`"mcc"`)
	keyIsOnline        = []byte(`"is_online"`)
	keyCardPresent     = []byte(`"card_present"`)
	keyKMFromHome      = []byte(`"km_from_home"`)
	keyTimestamp       = []byte(`"timestamp"`)
	keyKMFromCurrent   = []byte(`"km_from_current"`)
)

// findKey locates the first occurrence of key (already including its quotes,
// e.g. `"amount"`) in body[start:end] and returns the absolute offset.
// Returns -1 if not found.
func findKey(body []byte, start, end int, key []byte) int {
	if start >= end {
		return -1
	}
	idx := bytes.Index(body[start:end], key)
	if idx < 0 {
		return -1
	}
	return start + idx
}

// objectRange locates "key": { … } and returns [absolute_lbrace, absolute_rbrace_after_close).
// Returns ok=false if the value isn't an object (e.g. is null).
func objectRange(body []byte, start, end int, key []byte) (int, int, bool) {
	k := findKey(body, start, end, key)
	if k < 0 {
		return 0, 0, false
	}
	colon := indexByteFrom(body, k+len(key), end, ':')
	if colon < 0 {
		return 0, 0, false
	}
	p := skipWS(body, colon+1, end)
	if p >= end || body[p] != '{' {
		return 0, 0, false
	}
	close := matchBrace(body, p, end)
	if close < 0 {
		return 0, 0, false
	}
	return p, close, true
}

// jsonNumber parses key's numeric value as float32. Returns ok=false on missing key
// or unparseable number.
func jsonNumber(body []byte, start, end int, key []byte) (float32, bool) {
	k := findKey(body, start, end, key)
	if k < 0 {
		return 0, false
	}
	colon := indexByteFrom(body, k+len(key), end, ':')
	if colon < 0 {
		return 0, false
	}
	p := skipWS(body, colon+1, end)
	if p >= end {
		return 0, false
	}
	// Find the end of the number — a comma, brace, bracket, or whitespace.
	q := p
	if body[q] == '-' || body[q] == '+' {
		q++
	}
	for q < end {
		c := body[q]
		if (c >= '0' && c <= '9') || c == '.' || c == 'e' || c == 'E' || c == '-' || c == '+' {
			q++
			continue
		}
		break
	}
	if q == p {
		return 0, false
	}
	// unsafe.String avoids the string allocation that `string(body[p:q])` would
	// trigger on every numeric field (~10 per request). The body buffer is not
	// mutated during ParseFloat, and ParseFloat does not retain the string.
	v, err := strconv.ParseFloat(unsafe.String(&body[p], q-p), 32)
	if err != nil {
		return 0, false
	}
	return float32(v), true
}

func jsonBool(body []byte, start, end int, key []byte) (bool, bool) {
	k := findKey(body, start, end, key)
	if k < 0 {
		return false, false
	}
	colon := indexByteFrom(body, k+len(key), end, ':')
	if colon < 0 {
		return false, false
	}
	p := skipWS(body, colon+1, end)
	if p+4 <= end && body[p] == 't' && body[p+1] == 'r' && body[p+2] == 'u' && body[p+3] == 'e' {
		return true, true
	}
	if p+5 <= end && body[p] == 'f' && body[p+1] == 'a' && body[p+2] == 'l' && body[p+3] == 's' && body[p+4] == 'e' {
		return false, true
	}
	return false, false
}

// jsonString returns the inner-quoted bytes (no copy) for key's string value.
// Caller must not mutate the returned slice. Returns ok=false on missing key
// or non-string value.
func jsonString(body []byte, start, end int, key []byte) ([]byte, bool) {
	k := findKey(body, start, end, key)
	if k < 0 {
		return nil, false
	}
	colon := indexByteFrom(body, k+len(key), end, ':')
	if colon < 0 {
		return nil, false
	}
	p := skipWS(body, colon+1, end)
	if p >= end || body[p] != '"' {
		return nil, false
	}
	p++
	q := p
	for q < end && body[q] != '"' {
		q++
	}
	if q >= end {
		return nil, false
	}
	return body[p:q], true
}

// arrayContainsString returns true if key's array (a JSON list of strings)
// contains needle (compared by raw bytes).
func arrayContainsString(body []byte, start, end int, key, needle []byte) bool {
	k := findKey(body, start, end, key)
	if k < 0 {
		return false
	}
	colon := indexByteFrom(body, k+len(key), end, ':')
	if colon < 0 {
		return false
	}
	lb := indexByteFrom(body, colon+1, end, '[')
	if lb < 0 {
		return false
	}
	p := lb + 1
	for p < end {
		p = skipWS(body, p, end)
		if p >= end || body[p] == ']' {
			return false
		}
		if body[p] != '"' {
			p++
			continue
		}
		p++
		q := p
		for q < end && body[q] != '"' {
			q++
		}
		if q >= end {
			return false
		}
		if bytes.Equal(body[p:q], needle) {
			return true
		}
		p = q + 1
	}
	return false
}

func indexByteFrom(body []byte, start, end int, c byte) int {
	if start >= end {
		return -1
	}
	idx := bytes.IndexByte(body[start:end], c)
	if idx < 0 {
		return -1
	}
	return start + idx
}

func skipWS(body []byte, start, end int) int {
	for start < end {
		c := body[start]
		if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
			start++
			continue
		}
		return start
	}
	return end
}

// matchBrace assumes body[start] == '{' and returns the absolute index AFTER
// the matching '}'. Skips quoted strings and escaped chars.
func matchBrace(body []byte, start, end int) int {
	depth := 0
	inStr := false
	esc := false
	for p := start; p < end; p++ {
		c := body[p]
		if inStr {
			if esc {
				esc = false
				continue
			}
			if c == '\\' {
				esc = true
				continue
			}
			if c == '"' {
				inStr = false
			}
			continue
		}
		switch c {
		case '"':
			inStr = true
		case '{':
			depth++
		case '}':
			depth--
			if depth == 0 {
				return p + 1
			}
		}
	}
	return -1
}

func clamp01(x float32) float32 {
	if x < 0 {
		return 0
	}
	if x > 1 {
		return 1
	}
	return x
}

// isoHourUTC parses positions 11-12 of an ISO-8601 timestamp (e.g. "2026-03-11T20:23:35Z").
// Bounds-checked at the caller (timestamp is at least 19 chars).
func isoHourUTC(ts []byte) int {
	h := int(ts[11]-'0')*10 + int(ts[12]-'0')
	if h < 0 {
		return 0
	}
	if h > 23 {
		return 23
	}
	return h
}

// weekdayMondayZero returns 0 (Mon) .. 6 (Sun) for the date in ts.
// Uses Howard Hinnant's "days from civil" algorithm — pure integer math.
func weekdayMondayZero(ts []byte) int {
	y := int(ts[0]-'0')*1000 + int(ts[1]-'0')*100 + int(ts[2]-'0')*10 + int(ts[3]-'0')
	m := int(ts[5]-'0')*10 + int(ts[6]-'0')
	d := int(ts[8]-'0')*10 + int(ts[9]-'0')
	days := daysFromCivil(y, m, d)
	w := int((days + 3) % 7) // 1970-01-01 was a Thursday (=3 in mon-zero scheme)
	if w < 0 {
		w += 7
	}
	return w
}

// minutesBetween returns |a - b| in minutes. Both must be ISO-8601 to-second.
func minutesBetween(a, b []byte) int64 {
	sa := isoToEpochSeconds(a)
	sb := isoToEpochSeconds(b)
	d := sa - sb
	if d < 0 {
		d = -d
	}
	return d / 60
}

func isoToEpochSeconds(ts []byte) int64 {
	y := int(ts[0]-'0')*1000 + int(ts[1]-'0')*100 + int(ts[2]-'0')*10 + int(ts[3]-'0')
	m := int(ts[5]-'0')*10 + int(ts[6]-'0')
	d := int(ts[8]-'0')*10 + int(ts[9]-'0')
	hh := int(ts[11]-'0')*10 + int(ts[12]-'0')
	mm := int(ts[14]-'0')*10 + int(ts[15]-'0')
	ss := int(ts[17]-'0')*10 + int(ts[18]-'0')
	days := daysFromCivil(y, m, d)
	return days*86400 + int64(hh)*3600 + int64(mm)*60 + int64(ss)
}

// daysFromCivil — Howard Hinnant's algorithm. Returns days since 1970-01-01
// for a Gregorian (year, month, day). Works for all valid inputs.
func daysFromCivil(y, m, d int) int64 {
	if m <= 2 {
		y--
	}
	var era int
	if y >= 0 {
		era = y / 400
	} else {
		era = (y - 399) / 400
	}
	yoe := uint(y - era*400)
	var monthShift int
	if m > 2 {
		monthShift = -3
	} else {
		monthShift = 9
	}
	doy := uint((153*(uint(m)+uint(monthShift))+2)/5) + uint(d) - 1
	doe := yoe*365 + yoe/4 - yoe/100 + doy
	return int64(era)*146097 + int64(doe) - 719468
}
