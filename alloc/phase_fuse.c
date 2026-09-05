/* phase_fuse: обратимое FUSE-overlay (ReversibleFS-lite, #3).
 * Монтируется поверх каталога (env PHASE_FS_BACKING). Каждая мутация файла
 * (write/truncate/unlink/create) ПЕРЕД изменением сохраняет оригинал в
 * журнал; запись в виртуальный "/.phase_undo" откатывает ВСЕ изменения
 * (реверсивная семантика: файлы возвращаются бит-в-бит). "/.phase_audit"
 * отдаёт дайджест журнала (SINT-флёр).
 *
 *   gcc -D_FILE_OFFSET_BITS=64 $(pkg-config --cflags fuse3) phase_fuse.c \
 *       -o phase_fuse $(pkg-config --libs fuse3)
 *   PHASE_FS_BACKING=/tmp/back ./phase_fuse /tmp/mnt -f
 */
#define FUSE_USE_VERSION 31
#include <fuse3/fuse.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <dirent.h>

#define JCAP 1024
#define MAX_SAVE (16u<<20) /* 16 MiB максимум на файл-оригинал */
typedef struct { char path[512]; unsigned char* orig; size_t len; int existed; } J;
static J jlog[JCAP];
static int jn = 0;
static char backing[4096] = "/tmp/phase_fuse_backing";

static void fullpath(const char* p, char* out, size_t n){ snprintf(out, n, "%s%s", backing, p); }

/* ---------- журнал ---------- */
static int find_j(const char* p){ for(int i=0;i<jn;i++) if(!strcmp(jlog[i].path,p)) return i; return -1; }

/* сохранить оригинал файла (первый раз) */
static int save_orig(const char* p){
    if(find_j(p)>=0) return 0;
    if(jn>=JCAP) return -ENOSPC;
    char fp[4600]; fullpath(p,fp,sizeof fp);
    struct stat st; if(lstat(fp,&st)) return -errno;
    strcpy(jlog[jn].path,p);
    jlog[jn].existed = S_ISREG(st.st_mode);
    jlog[jn].orig=NULL; jlog[jn].len=0;
    if(jlog[jn].existed){
        if(st.st_size > (off_t)MAX_SAVE) return -EFBIG;
        FILE* f=fopen(fp,"rb"); if(!f) return -errno;
        jlog[jn].orig=malloc(st.st_size? (size_t)st.st_size:1);
        if(!jlog[jn].orig){ fclose(f); return -ENOMEM; }
        jlog[jn].len=(size_t)fread(jlog[jn].orig,1,(size_t)st.st_size,f);
        fclose(f);
    }
    jn++;
    return 0;
}

static void undo_all(void){
    for(int i=jn-1;i>=0;i--){
        char fp[4600]; fullpath(jlog[i].path,fp,sizeof fp);
        if(jlog[i].existed){
            FILE* f=fopen(fp,"wb");
            if(f){ if(jlog[i].len) fwrite(jlog[i].orig,1,jlog[i].len,f); fclose(f); }
        } else {
            unlink(fp);
        }
        free(jlog[i].orig); jlog[i].orig=NULL;
    }
    jn=0;
}

/* ---------- виртуальные файлы ---------- */
static int is_undo(const char* p){ return !strcmp(p,"/.phase_undo"); }
static int is_audit(const char* p){ return !strcmp(p,"/.phase_audit"); }

static unsigned long fnv(const unsigned char* d, size_t n){
    unsigned long h=0xcbf29ce484222325ull;
    for(size_t i=0;i<n;i++){ h^=d[i]; h*=0x100000001b3ull; }
    return h;
}

/* ---------- fuse callbacks ---------- */
static int ph_getattr(const char* p, struct stat* st, struct fuse_file_info* fi){
    (void)fi;
    memset(st,0,sizeof *st);
    if(is_undo(p)||is_audit(p)){ st->st_mode=S_IFREG|0444; st->st_nlink=1; st->st_size=64; return 0; }
    char fp[4600]; fullpath(p,fp,sizeof fp);
    if(lstat(fp,st)) return -errno;
    return 0;
}

static int ph_readdir(const char* p, void* buf, fuse_fill_dir_t fill, off_t off,
                      struct fuse_file_info* fi, enum fuse_readdir_flags fl){
    (void)off;(void)fi;(void)fl;
    char fp[4600]; fullpath(p,fp,sizeof fp);
    DIR* d=opendir(fp); if(!d) return -errno;
    struct dirent* e;
    while((e=readdir(d))) if(fill(buf,e->d_name,NULL,0,0)) break;
    closedir(d);
    return 0;
}

static int ph_open(const char* p, struct fuse_file_info* fi){
    if(is_undo(p)||is_audit(p)){ fi->fh=0; return 0; }
    char fp[4600]; fullpath(p,fp,sizeof fp);
    int fd=open(fp,fi->flags); if(fd<0) return -errno;
    fi->fh=fd; return 0;
}

static int ph_read(const char* p, char* buf, size_t sz, off_t off, struct fuse_file_info* fi){
    if(is_audit(p)){
        char tmp[512]; int n=snprintf(tmp,sizeof tmp,"journal=%d digest=0x%08lx\n",jn,fnv((unsigned char*)tmp,0));
        /* осмысленный дайджест: хэш путей+длин журнала */
        unsigned long h=1469598103934665603ul;
        for(int i=0;i<jn;i++){ h^=fnv((unsigned char*)jlog[i].path,strlen(jlog[i].path)); h*=1099511628211ul; h^=(unsigned long)jlog[i].len; }
        n=snprintf(tmp,sizeof tmp,"journal=%d digest=0x%016lx\n",jn,h);
        if((size_t)off>=(size_t)n) return 0;
        size_t c=n-(size_t)off; if(c>sz) c=sz;
        memcpy(buf,tmp+off,c); return (int)c;
    }
    if(is_undo(p)) return 0;
    return pread((int)fi->fh,buf,sz,off);
}

static int ph_write(const char* p, const char* buf, size_t sz, off_t off, struct fuse_file_info* fi){
    if(is_undo(p)){ undo_all(); return (int)sz; }
    int r=save_orig(p); if(r<0) return r;
    return pwrite((int)fi->fh,buf,sz,off);
}

static int ph_truncate(const char* p, off_t sz, struct fuse_file_info* fi){
    (void)fi;
    int r=save_orig(p); if(r<0) return r;
    char fp[4600]; fullpath(p,fp,sizeof fp);
    if(truncate(fp,sz)) return -errno;
    return 0;
}

static int ph_unlink(const char* p){
    if(is_undo(p)||is_audit(p)) return -EPERM;
    int r=save_orig(p); if(r<0) return r;
    char fp[4600]; fullpath(p,fp,sizeof fp);
    if(unlink(fp)) return -errno;
    return 0;
}

static int ph_create(const char* p, mode_t m, struct fuse_file_info* fi){
    char fp[4600]; fullpath(p,fp,sizeof fp);
    int fd=open(fp,O_CREAT|O_WRONLY|O_TRUNC,m);
    if(fd<0) return -errno;
    /* журналируем как созданный (undo удалит) */
    if(find_j(p)<0 && jn<JCAP){
        strcpy(jlog[jn].path,p); jlog[jn].orig=NULL; jlog[jn].len=0; jlog[jn].existed=0; jn++;
    }
    fi->fh=fd; return 0;
}

static int ph_release(const char* p, struct fuse_file_info* fi){
    if(!is_undo(p)&&!is_audit(p)&&fi->fh>0) close((int)fi->fh);
    return 0;
}

static struct fuse_operations oper = {
    .getattr = ph_getattr,
    .readdir = ph_readdir,
    .open = ph_open,
    .read = ph_read,
    .write = ph_write,
    .truncate = ph_truncate,
    .unlink = ph_unlink,
    .create = ph_create,
    .release = ph_release,
};

int main(int argc, char** argv){
    const char* b=getenv("PHASE_FS_BACKING");
    if(b) snprintf(backing,sizeof backing,"%s",b);
    return fuse_main(argc,argv,&oper,NULL);
}
