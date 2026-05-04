SHELL := /bin/bash
ROOT := $(abspath ..)
RES  := $(ROOT)/resources
BUILD_RES := build_resources

.PHONY: help resources index preprocess build up down logs smoke clean

help:
	@echo "make targets:"
	@echo "  index       — generate ../resources/index.bin from references.json.gz (~120s)"
	@echo "  resources   — copy ../resources/{index.bin,mcc_risk.json} into api/build_resources/"
	@echo "  build       — docker compose build (calls resources first)"
	@echo "  up          — docker compose up -d --build"
	@echo "  down        — docker compose down"
	@echo "  logs        — docker compose logs -f"
	@echo "  smoke       — curl smoke against http://localhost:9999"
	@echo "  clean       — remove api/build_resources and the prebuilt index"

# Generate the IVF6 index from references.json.gz. Idempotent — overwrites
# the existing file. ~120s on a modern Mac.
index:
	cd api && go build -o /tmp/sketchy-preprocess ./cmd/preprocess
	/tmp/sketchy-preprocess $(RES)/references.json.gz $(RES)/index.bin

# Stage the index + mcc_risk.json inside the docker build context.
resources:
	mkdir -p $(BUILD_RES)
	cp $(RES)/index.bin $(BUILD_RES)/index.bin
	cp $(RES)/mcc_risk.json $(BUILD_RES)/mcc_risk.json

build: resources
	docker compose build

up: resources
	docker compose up -d --build

down:
	docker compose down

logs:
	docker compose logs -f --tail=200

smoke:
	@echo "GET /ready:"
	@curl -fsS http://localhost:9999/ready -o /dev/null -w "  HTTP %{http_code}\n"
	@echo "POST /fraud-score (legit):"
	@curl -fsS -XPOST http://localhost:9999/fraud-score \
		-H 'Content-Type: application/json' \
		-d '{"id":"tx-1329056812","transaction":{"amount":41.12,"installments":2,"requested_at":"2026-03-11T18:45:53Z"},"customer":{"avg_amount":82.24,"tx_count_24h":3,"known_merchants":["MERC-003","MERC-016"]},"merchant":{"id":"MERC-016","mcc":"5411","avg_amount":60.25},"terminal":{"is_online":false,"card_present":true,"km_from_home":29.23},"last_transaction":null}'
	@echo
	@echo "POST /fraud-score (fraud):"
	@curl -fsS -XPOST http://localhost:9999/fraud-score \
		-H 'Content-Type: application/json' \
		-d '{"id":"tx-3330991687","transaction":{"amount":9505.97,"installments":10,"requested_at":"2026-03-14T05:15:12Z"},"customer":{"avg_amount":81.28,"tx_count_24h":20,"known_merchants":["MERC-008","MERC-007","MERC-005"]},"merchant":{"id":"MERC-068","mcc":"7802","avg_amount":54.86},"terminal":{"is_online":false,"card_present":true,"km_from_home":952.27},"last_transaction":null}'
	@echo

clean:
	rm -rf $(BUILD_RES)
	rm -f $(RES)/index.bin
