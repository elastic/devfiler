.PHONY: all release debug init test clean

CARGO = cargo

# Set V=1 for verbose output
ifeq ($(V),1)
	Q = 
else
	Q = @
endif

all: release

release:
	$(Q)$(CARGO) build --release

debug:
	$(Q)$(CARGO) build --debug

init:
	$(Q)git submodule init
	$(Q)git submodule update

test:
	$(Q)$(CARGO) test

clean:
	$(Q)$(CARGO) clean

