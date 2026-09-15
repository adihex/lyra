# Lyra — no-xcodeproj build. Rust core → staticlib → swiftc → .app bundle → codesign.
# For release we'd move to XcodeGen for Sparkle's nested-signing (see BLUEPRINT § packaging).

CARGO    := mise exec -- cargo
SWIFTC   := xcrun swiftc
APP      := .build/Lyra.app
APPBIN   := $(APP)/Contents/MacOS/Lyra
LIBDIR   := app/.libs
SWIFT_SRC:= $(wildcard app/Sources/LyraApp/*.swift)

.PHONY: all rust app sign run clean check

all: app

check:
	$(CARGO) check --workspace

rust:
	$(CARGO) build -p lyra-ffi
	@mkdir -p $(LIBDIR)
	@cp target/debug/liblyra_ffi.a $(LIBDIR)/

app: rust $(APPBIN) $(APP)/Contents/Info.plist sign

$(APPBIN): $(SWIFT_SRC) $(LIBDIR)/liblyra_ffi.a
	@mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	$(SWIFTC) -O -parse-as-library \
	    -import-objc-header modules/CLyraFFI/lyra.h \
	    -I modules/CLyraFFI \
	    $(SWIFT_SRC) \
	    -L $(LIBDIR) -llyra_ffi \
	    -framework SwiftUI -framework AppKit -framework UniformTypeIdentifiers \
	    -framework AudioToolbox -framework CoreAudio -framework CoreFoundation \
	    -framework AVFoundation -framework CoreServices \
	    -o $(APPBIN)

$(APP)/Contents/Info.plist: Info.plist
	@cp Info.plist $(APP)/Contents/

sign: $(APPBIN)
	codesign --force --sign - --entitlements entitlements.plist $(APP)

run: app
	open $(APP)

clean:
	rm -rf .build $(LIBDIR)
	$(CARGO) clean
