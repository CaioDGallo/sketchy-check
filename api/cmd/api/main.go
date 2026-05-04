// api is the runtime entry point for a single sketchy backend instance.
// It loads the IVF6 index and the MCC risk table at startup, pre-builds the
// HTTP response set, then serves /fraud-score and /ready over a Unix Domain
// Socket until SIGTERM/SIGINT.
package main

import (
	"fmt"
	"log"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"

	"github.com/rinha2026/sketchy/api/internal/ivf"
	"github.com/rinha2026/sketchy/api/internal/mcc"
	"github.com/rinha2026/sketchy/api/internal/responses"
	"github.com/rinha2026/sketchy/api/internal/server"
)

func main() {
	indexPath := envDefault("INDEX_PATH", "/app/index.bin")
	mccPath := envDefault("MCC_RISK_PATH", "/app/mcc_risk.json")
	udsPath := envDefault("UDS_PATH", "/sockets/api.sock")
	nprobe := envInt("IVF_NPROBE", 1)
	profileEvery := envInt("SKETCHY_PROFILE_EVERY", 0)

	responses.Init()

	if err := mcc.Load(mccPath); err != nil {
		log.Printf("mcc load: %v (continuing with defaults)", err)
	}

	t0 := time.Now()
	idx, err := ivf.Load(indexPath)
	if err != nil {
		log.Fatalf("index load: %v", err)
	}
	log.Printf("index IVF6 loaded: N=%d K=%d in %s", idx.N, ivf.K, time.Since(t0))

	ln, err := server.Run(server.Config{
		UDSPath:      udsPath,
		Index:        idx,
		SearchOpts:   ivf.SearchOpts{NProbe: nprobe},
		ProfileEvery: profileEvery,
	})
	if err != nil {
		log.Fatalf("server: %v", err)
	}
	log.Printf("listening on %s nprobe=%d", udsPath, nprobe)

	// Block until SIGTERM. The k8s/docker stop sends SIGTERM by default.
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop
	log.Printf("shutdown signal received, closing listener")
	ln.Close()
	// Best-effort UDS file removal so the next start has a clean slate.
	_ = os.Remove(udsPath)
}

func envDefault(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

func envInt(name string, def int) int {
	v := os.Getenv(name)
	if v == "" {
		return def
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		fmt.Fprintf(os.Stderr, "warn: env %s=%q is not an int, using default %d\n", name, v, def)
		return def
	}
	return n
}
