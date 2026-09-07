CC?=cc
all: galloc.a demo
galloc.a: galloc.c galloc.h
	$(CC) -O2 -c -o galloc.o galloc.c
	ar rcs galloc.a galloc.o
demo: game_demo.c galloc.a galloc.h
	$(CC) -O2 -pthread -o demo game_demo.c galloc.a
clean:
	rm -f galloc.o galloc.a demo
