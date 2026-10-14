CFLAGS += -std=gnu11 -O2 -Wall -Wextra -Werror $(shell pkg-config --cflags libpipewire-0.3)
LDLIBS += $(shell pkg-config --libs libpipewire-0.3) -lm
all: build/mixyard-pw
build/mixyard-pw: mixyard_pw.c
	@pkg-config --atleast-version=1.6 libpipewire-0.3
	mkdir -p build
	$(CC) $(CFLAGS) $< -o $@ $(LDLIBS)
clean:
	rm -f build/mixyard-pw
.PHONY: all clean
