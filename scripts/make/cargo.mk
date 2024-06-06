# Cargo features and build args

ifeq ($(V),1)
  verbose := -v
else ifeq ($(V),2)
  verbose := -vv
else
  verbose :=
endif

ifeq ($(shell test $(SMP) -gt 1; echo $$?),0)
  features-y += axstd/smp
endif

ifneq ($(filter $(LOG),off error warn info debug trace),)
  features-y += axstd/log-level-$(LOG)
else
  $(error "LOG" must be one of "off", "error", "warn", "info", "debug", "trace")
endif

ifeq ($(GIC_V3), y)
  features-y += axstd/gic_v3
endif



features-$(FS)        += axstd/fs
features-$(NET)       += axstd/net
features-$(GRAPHIC)   += axstd/display
features-$(HV)        += axstd/hv 


build_args-release := --release

build_args := \
  --target $(TARGET) \
  --target-dir $(CURDIR)/target \
  $(build_args-$(MODE)) \
  --features "$(features-y)" \
   --bin $(BIN_TYPE)\
  $(verbose)

RUSTFLAGS := -C link-arg=-T$(LD_SCRIPT) -C link-arg=-no-pie
RUSTDOCFLAGS := --enable-index-page -Zunstable-options -D rustdoc::broken_intra_doc_links

ifeq ($(ARCH), x86_64)
  RUSTFLAGS += -C link-arg=--no-relax
endif

ifeq ($(MAKECMDGOALS), doc_check_missing)
  RUSTDOCFLAGS += -D missing-docs
endif

define cargo_build
  $(call run_cmd,cargo build,$(build_args) $(1) --features "$(strip $(2))")
endef

define cargo_clippy
  $(call run_cmd,cargo clippy,--all-features --workspace --exclude axlog $(1) $(verbose))
  $(call run_cmd,cargo clippy,-p axlog -p percpu -p percpu_macros $(1) $(verbose))
endef

all_packages := \
  $(shell ls $(CURDIR)/crates) \
  $(shell ls $(CURDIR)/modules) \
  axfeat arceos_api axstd axlibc axstarry

define cargo_doc
  $(call run_cmd,cargo doc,--no-deps --all-features --workspace --exclude "arceos-*" $(verbose))
  @# run twice to fix broken hyperlinks
  $(foreach p,$(all_packages), \
    $(call run_cmd,cargo rustdoc,--all-features -p $(p) $(verbose))
  )
  @# for some crates, re-generate without `--all-features`
  $(call run_cmd,cargo doc,--no-deps -p percpu $(verbose))
endef
