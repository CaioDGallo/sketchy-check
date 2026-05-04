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

var risks [10000]float32
var riskSet [10000]bool

func init() {
	loadDefaults()
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
	loadDefaults()
	for k, v := range raw {
		if code, ok := code4String(k); ok {
			risks[code] = v
			riskSet[code] = true
		}
	}
	return nil
}

// Get returns the risk for an MCC byte slice, or DefaultRisk if unknown.
func Get(mcc []byte) float32 {
	if code, ok := code4Bytes(mcc); ok && riskSet[code] {
		return risks[code]
	}
	return DefaultRisk
}

func loadDefaults() {
	for i := range riskSet {
		riskSet[i] = false
		risks[i] = 0
	}
	setDefault(5411, 0.15)
	setDefault(5812, 0.30)
	setDefault(5912, 0.20)
	setDefault(5944, 0.45)
	setDefault(7801, 0.80)
	setDefault(7802, 0.75)
	setDefault(7995, 0.85)
	setDefault(4511, 0.35)
	setDefault(5311, 0.25)
	setDefault(5999, 0.50)
}

func setDefault(code int, risk float32) {
	risks[code] = risk
	riskSet[code] = true
}

func code4Bytes(mcc []byte) (int, bool) {
	if len(mcc) < 4 {
		return 0, false
	}
	c0, c1, c2, c3 := mcc[0], mcc[1], mcc[2], mcc[3]
	if c0 < '0' || c0 > '9' || c1 < '0' || c1 > '9' || c2 < '0' || c2 > '9' || c3 < '0' || c3 > '9' {
		return 0, false
	}
	return int(c0-'0')*1000 + int(c1-'0')*100 + int(c2-'0')*10 + int(c3-'0'), true
}

func code4String(mcc string) (int, bool) {
	if len(mcc) < 4 {
		return 0, false
	}
	c0, c1, c2, c3 := mcc[0], mcc[1], mcc[2], mcc[3]
	if c0 < '0' || c0 > '9' || c1 < '0' || c1 > '9' || c2 < '0' || c2 > '9' || c3 < '0' || c3 > '9' {
		return 0, false
	}
	return int(c0-'0')*1000 + int(c1-'0')*100 + int(c2-'0')*10 + int(c3-'0'), true
}
