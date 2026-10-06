ifndef IN_NIX_SHELL
$(error Use nix develop -c make -C native)
endif
# arm64e needs no compiler builtins here; Nix's bundled arm64 runtime is a different ABI.
CFLAGS = -Wall -Wextra -Werror -O2 -g -mmacosx-version-min=14.0 -nodefaultlibs
LDLIBS = -lSystem -lobjc -framework Cocoa
.PHONY: all clean
all: build/ribbon-payload.dylib build/dock-loader
build:
	mkdir -p build
build/ribbon-payload.dylib: payload.m skylight.h Makefile | build
	$(CC) $(CFLAGS) -fobjc-arc -arch arm64e -dynamiclib payload.m $(LDLIBS) -o $@
	/usr/bin/codesign -s - -f $@
build/dock-loader: loader.m Makefile | build
	$(CC) $(CFLAGS) -Wno-unused-parameter -arch arm64e loader.m $(LDLIBS) -o $@
	/usr/bin/codesign -s - -f $@
build/test-window: test-window.m query.m events.m input.m bridge.h skylight.h Makefile | build
	$(CC) $(CFLAGS) -fobjc-arc test-window.m query.m events.m input.m $(LDLIBS) -framework ApplicationServices -o $@
build/socket-test: socket-test.m payload.m skylight.h Makefile | build
	$(CC) $(CFLAGS) -fobjc-arc socket-test.m $(LDLIBS) -o $@
build/interactive-test: interactive-test.m payload.m skylight.h Makefile | build
	$(CC) $(CFLAGS) -fobjc-arc interactive-test.m $(LDLIBS) -o $@
build/input-test: input-test.m input.m bridge.h Makefile | build
	$(CC) $(CFLAGS) -fobjc-arc input-test.m input.m $(LDLIBS) -o $@
clean:
	rm -rf build
