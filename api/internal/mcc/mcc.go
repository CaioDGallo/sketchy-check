// Package mcc holds the merchant-category risk lookup used by index 12 of
// the fraud vector. Built-in defaults match resources/mcc_risk.json and can
// be overridden at startup via Load.
package mcc

import (
	"encoding/json"
	"os"
)

// DefaultRisk is returned for any MCC not present in the loaded table.
const DefaultRisk float32 = 0.5

// table holds the risk per 4-char MCC. Read-only after init.
//
// Mirrors resources/mcc_risk.json so the binary works even if the JSON file
// is missing at runtime — the test environment always provides it but local
// dev runs without it should not crash.
var table = map[string]float32{
	"5411": 0.15,
	"5812": 0.30,
	"5912": 0.20,
	"5944": 0.45,
	"7801": 0.80,
	"7802": 0.75,
	"7995": 0.85,
	"4511": 0.35,
	"5311": 0.25,
	"5999": 0.50,
}

// Load replaces the in-memory table with entries from path (a JSON object
// mapping MCC string → risk float). Missing path is non-fatal — defaults stay
// in effect.
func Load(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	var raw map[string]float32
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}
	if len(raw) == 0 {
		return nil
	}
	out := make(map[string]float32, len(raw))
	for k, v := range raw {
		out[k] = v
	}
	table = out
	return nil
}

// Get returns the risk for an MCC byte slice, or DefaultRisk if unknown.
// Takes a byte slice to avoid an allocation for the lookup string in the hot path
// — Go's compiler converts a []byte to string with a single ABI conversion when
// used as a map key, no heap allocation.
func Get(mcc []byte) float32 {
	if v, ok := table[string(mcc)]; ok {
		return v
	}
	return DefaultRisk
}
