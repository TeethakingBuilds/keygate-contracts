# --- keygate-contracts -------------------------------------------------------
# Common tasks. CI runs the same commands, so `make check` should be a proxy
# for what a pull request will hit.

CARGO ?= cargo
CONTRACTS := keygate-core keygate-policy keygate-circuit-breaker keygate-multisig

.PHONY: all build test check fmt fmt-check clippy optimize clean

all: check

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Release build of every contract as stripped wasm, ready to deploy.
optimize:
	$(CARGO) build --release --target wasm32-unknown-unknown \
		$(addprefix -p ,$(CONTRACTS))

check: fmt-check clippy test

clean:
	$(CARGO) clean
