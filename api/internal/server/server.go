// Package server implements a minimal HTTP/1.1 server over a Unix Domain
// Socket. It bypasses net/http entirely — the request shapes are fixed and
// the latency budget cannot afford net/http's reflection-driven router.
//
// Hot path:
//
//	read → find "\r\n\r\n" → parse method + path + Content-Length → vectorize → search → write
//
// One goroutine per connection. With nginx upstream keepalive, a small
// pool of long-lived connections drives many requests through each goroutine.
package server

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"strconv"
	"sync/atomic"
	"time"
	"unsafe"

	"github.com/rinha2026/sketchy/api/internal/ivf"
	"github.com/rinha2026/sketchy/api/internal/responses"
	"github.com/rinha2026/sketchy/api/internal/vectorize"
)

const (
	maxRequestSize = 32 * 1024 // 32 KiB; the longest legitimate payload is < 2 KiB
)

// Config wires the server to its dependencies.
type Config struct {
	UDSPath      string
	Index        *ivf.Index
	SearchOpts   ivf.SearchOpts
	ProfileEvery int
}

// Run binds to UDSPath and serves until the listener is closed. Returns the
// listener so the caller can close it for graceful shutdown.
func Run(cfg Config) (net.Listener, error) {
	// Best-effort cleanup in case a previous instance left a stale socket.
	_ = os.Remove(cfg.UDSPath)

	ln, err := net.Listen("unix", cfg.UDSPath)
	if err != nil {
		return nil, fmt.Errorf("listen %s: %w", cfg.UDSPath, err)
	}
	if err := os.Chmod(cfg.UDSPath, 0o666); err != nil {
		ln.Close()
		return nil, fmt.Errorf("chmod %s: %w", cfg.UDSPath, err)
	}

	prof := newProfiler(cfg.ProfileEvery)
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				if errors.Is(err, net.ErrClosed) {
					return
				}
				continue
			}
			go handle(conn, cfg.Index, cfg.SearchOpts, prof)
		}
	}()
	return ln, nil
}

type profiler struct {
	every           uint64
	requests        atomic.Uint64
	vectorizeNs     atomic.Uint64
	searchNs        atomic.Uint64
	writeNs         atomic.Uint64
	scannedClusters atomic.Uint64
	scannedVectors  atomic.Uint64
}

func newProfiler(every int) *profiler {
	if every <= 0 {
		return nil
	}
	log.Printf("profile enabled: logging cumulative averages every %d requests", every)
	return &profiler{every: uint64(every)}
}

func (p *profiler) record(vectorizeDur, searchDur, writeDur time.Duration, stats ivf.SearchStats) {
	n := p.requests.Add(1)
	p.vectorizeNs.Add(uint64(vectorizeDur))
	p.searchNs.Add(uint64(searchDur))
	p.writeNs.Add(uint64(writeDur))
	p.scannedClusters.Add(uint64(stats.ScannedClusters))
	p.scannedVectors.Add(uint64(stats.ScannedVectors))
	if n%p.every != 0 {
		return
	}
	requests := float64(n)
	log.Printf(
		"profile req=%d avg_vectorize=%s avg_search=%s avg_write=%s avg_clusters=%.1f avg_vectors=%.0f",
		n,
		time.Duration(p.vectorizeNs.Load()/n),
		time.Duration(p.searchNs.Load()/n),
		time.Duration(p.writeNs.Load()/n),
		float64(p.scannedClusters.Load())/requests,
		float64(p.scannedVectors.Load())/requests,
	)
}

// handle serves all requests on a single keep-alive connection. The buffer is
// allocated once per connection — across requests on the same connection,
// data is shifted left after each request rather than reallocated.
func handle(conn net.Conn, idx *ivf.Index, opts ivf.SearchOpts, prof *profiler) {
	defer conn.Close()
	buf := make([]byte, maxRequestSize)
	var q [14]float32
	used := 0

	for {
		// Ensure we have at least one full request in `buf[:used]`.
		// Loop reading from conn until headers are complete and body is fully present.
		var headerEnd, contentLength, bodyStart, msgEnd int
		var closeAfter bool
		var resp []byte
		var ready bool
		var recordProfile bool
		var vectorizeDur, searchDur time.Duration
		var stats ivf.SearchStats

		for !ready {
			if used >= len(buf) {
				// Request bigger than our buffer ceiling — 413 and close.
				resp = responses.TooLarge
				closeAfter = true
				ready = true
				break
			}
			n, err := conn.Read(buf[used:])
			if err != nil {
				if err == io.EOF || used == 0 {
					return
				}
				return
			}
			used += n

			// Look for end of headers.
			headerEnd = bytes.Index(buf[:used], []byte("\r\n\r\n"))
			if headerEnd < 0 {
				continue
			}
			bodyStart = headerEnd + 4

			// Parse method + path from the request line.
			lineEnd := bytes.Index(buf[:headerEnd], []byte("\r\n"))
			if lineEnd < 0 {
				resp = responses.BadRequest
				closeAfter = true
				ready = true
				break
			}
			line := buf[:lineEnd]

			// Fast path: GET /ready.
			if bytes.HasPrefix(line, []byte("GET /ready")) {
				resp = responses.Ready
				msgEnd = bodyStart
				ready = true
				break
			}

			// POST /fraud-score
			if !bytes.HasPrefix(line, []byte("POST /fraud-score")) {
				resp = responses.NotFound
				closeAfter = true
				msgEnd = bodyStart
				ready = true
				break
			}

			// Find Content-Length within the headers.
			cl, ok := parseContentLength(buf[lineEnd+2 : headerEnd])
			if !ok {
				resp = responses.BadRequest
				closeAfter = true
				msgEnd = bodyStart
				ready = true
				break
			}
			contentLength = cl
			if contentLength < 0 || contentLength > maxRequestSize-bodyStart {
				resp = responses.TooLarge
				closeAfter = true
				msgEnd = bodyStart
				ready = true
				break
			}

			// Need the full body.
			if used-bodyStart < contentLength {
				continue
			}
			msgEnd = bodyStart + contentLength

			body := buf[bodyStart:msgEnd]
			if prof == nil {
				if err := vectorize.Build(body, &q); err != nil {
					resp = responses.BadRequest
					closeAfter = true
				} else {
					frauds := idx.Search(&q, opts)
					if frauds < 0 || frauds > 5 {
						resp = responses.ServerError
						closeAfter = true
					} else {
						resp = responses.FraudScore[frauds]
					}
				}
				ready = true
				continue
			}

			startVectorize := time.Now()
			recordProfile = true
			if err := vectorize.Build(body, &q); err != nil {
				vectorizeDur = time.Since(startVectorize)
				resp = responses.BadRequest
				closeAfter = true
			} else {
				vectorizeDur = time.Since(startVectorize)
				startSearch := time.Now()
				frauds, searchStats := idx.SearchWithStats(&q, opts)
				searchDur = time.Since(startSearch)
				stats = searchStats
				if frauds < 0 || frauds > 5 {
					resp = responses.ServerError
					closeAfter = true
				} else {
					resp = responses.FraudScore[frauds]
				}
			}
			ready = true
		}

		if prof == nil {
			if _, err := conn.Write(resp); err != nil {
				return
			}
		} else {
			startWrite := time.Now()
			if _, err := conn.Write(resp); err != nil {
				return
			}
			if recordProfile {
				prof.record(vectorizeDur, searchDur, time.Since(startWrite), stats)
			}
		}
		if closeAfter {
			return
		}

		// Shift any buffered bytes belonging to the next request to the front.
		if msgEnd < used {
			copy(buf, buf[msgEnd:used])
			used -= msgEnd
		} else {
			used = 0
		}
	}
}

// parseContentLength scans header bytes (between request line and the empty line)
// for "Content-Length:" (case-insensitive) and parses the integer value.
func parseContentLength(headers []byte) (int, bool) {
	// Lower-case scan: "Content-Length" appears once and is short.
	const target = "content-length:"
	// Walk header lines.
	for len(headers) > 0 {
		end := bytes.Index(headers, []byte("\r\n"))
		var line []byte
		if end < 0 {
			line = headers
			headers = nil
		} else {
			line = headers[:end]
			headers = headers[end+2:]
		}
		if len(line) < len(target) {
			continue
		}
		if !hasHeaderPrefix(line, target) {
			continue
		}
		v := line[len(target):]
		// Trim leading whitespace.
		for len(v) > 0 && (v[0] == ' ' || v[0] == '\t') {
			v = v[1:]
		}
		// Parse digits — zero-copy via unsafe.String. v is a re-slice of the
		// per-connection request buffer, which Atoi only reads.
		for len(v) > 0 && (v[len(v)-1] == ' ' || v[len(v)-1] == '\t') {
			v = v[:len(v)-1]
		}
		if len(v) == 0 {
			return 0, false
		}
		n, err := strconv.Atoi(unsafe.String(&v[0], len(v)))
		if err != nil {
			return 0, false
		}
		return n, true
	}
	return 0, false
}

func hasHeaderPrefix(line []byte, target string) bool {
	if len(line) < len(target) {
		return false
	}
	for i := 0; i < len(target); i++ {
		c := line[i]
		if c >= 'A' && c <= 'Z' {
			c += 'a' - 'A'
		}
		if c != target[i] {
			return false
		}
	}
	return true
}
