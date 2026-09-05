CC?=cc
all: libphase_alloc.so bench
libphase_alloc.so: phase_alloc.c
	$(CC) -O2 -shared -fPIC -o $@ phase_alloc.c
bench: bench.c
	$(CC) -O2 -o $@ bench.c
api: api.c
	cc -O2 -o $@ api.c
stress: stress.c
	cc -O2 -pthread -o $@ stress.c
clean:
	rm -f libphase_alloc.so bench api stress
