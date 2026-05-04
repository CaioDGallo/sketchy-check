// Package responses pre-builds the small set of HTTP responses the server can
// return. The fraud-score endpoint has only six possible bodies — one per
// fraud count in {0..5} — so we format them once at startup and emit the
// pre-built byte slice on each request.
//
// This is the same trick the reference C solution uses: zero JSON marshaling
// in the hot path, just a memcpy.
package responses

import (
	"fmt"
)

// FraudScore[i] is the full HTTP/1.1 200 response (status line + headers + body)
// for `i` fraud votes among the top-5. The decision rule (matches the reference):
//
//	score    = i * 0.2
//	approved = i < 3
//
// Mapping:
//
//	0 → score 0.0, approved
//	1 → score 0.2, approved
//	2 → score 0.4, approved
//	3 → score 0.6, denied
//	4 → score 0.8, denied
//	5 → score 1.0, denied
//
// Threshold (frauds < 3 ↔ score < 0.6) matches DETECTION_RULES.md exactly.
var FraudScore [6][]byte

// Ready is the response for GET /ready.
var Ready []byte

// Status responses for error paths. NotFound, BadRequest, etc.
var (
	NotFound    []byte
	BadRequest  []byte
	TooLarge    []byte
	ServerError []byte
)

// Init must be called once at process startup before the server begins
// serving traffic.
func Init() {
	for i := 0; i <= 5; i++ {
		score := float32(i) * 0.2
		approved := "true"
		if i >= 3 {
			approved = "false"
		}
		body := fmt.Sprintf(`{"approved":%s,"fraud_score":%.4f}`, approved, score)
		FraudScore[i] = build200(body)
	}
	Ready = []byte("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n")
	NotFound = []byte("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
	BadRequest = []byte("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"error\":\"invalid_payload\"}")
	TooLarge = []byte("HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
	ServerError = []byte("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

func build200(body string) []byte {
	return []byte(fmt.Sprintf(
		"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: keep-alive\r\n\r\n%s",
		len(body), body,
	))
}
