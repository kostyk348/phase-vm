CC?=cc
all: libphase_alloc.so bench
libphase_alloc.so: phase_alloc.c
	$(CC) -O2 -shared -fPIC -o $@ phase_alloc.c
bench: bench.c
	$(CC) -O2 -o $@ bench.c
clean:
	rm -f libphase_alloc.so bench
