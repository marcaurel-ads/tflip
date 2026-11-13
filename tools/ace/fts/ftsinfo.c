/* Dump AdsGetFTSIndexInfo for the committed fts fixture tables, to learn each
 * index's parsing options and to spec the function. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
typedef unsigned long U32; typedef unsigned short U16; typedef unsigned char U8; typedef ULONG_PTR H;
static HMODULE ace;
#define F(n) GetProcAddress(ace, n)
static void info(H t, const char*file){
  U16 n=0; ((U32(WINAPI*)(H,U16*))F("AdsGetNumIndexes"))(t,&n);
  U16 fn=0; ((U32(WINAPI*)(H,U16*))F("AdsGetNumFTSIndexes"))(t,&fn);
  printf("%s: %u indexes, %u fts\n",file,n,fn);
  for(U16 i=1;i<=(U16)(n+fn+2);i++){
    H idx=0; if(((U32(WINAPI*)(H,U16,H*))F("AdsGetIndexHandleByOrder"))(t,i,&idx))continue;
    U8 out[4096]; U32 len=sizeof out; U8 *field=0,*del=0,*noise=0,*drop=0,*cond=0,*r1=0,*r2=0;
    U32 mn=0,mx=0,opt=0;
    U32 rc=((U32(WINAPI*)(H,U8*,U32*,U8**,U32*,U32*,U8**,U8**,U8**,U8**,U8**,U8**,U32*))
      F("AdsGetFTSIndexInfo"))(idx,out,&len,&field,&mn,&mx,&del,&noise,&drop,&cond,&r1,&r2,&opt);
    if(rc)continue;
    U8 nm[128]; U16 nl=sizeof nm;
    ((U32(WINAPI*)(H,U8*,U16*))F("AdsGetIndexName"))(idx,nm,&nl);
    printf("%s order %lu tag=%s:\n",file,(unsigned long)i,nm);
    printf("  field=<%s> min=%lu max=%lu options=0x%lx\n",field?(char*)field:"",(unsigned long)mn,(unsigned long)mx,(unsigned long)opt);
    printf("  delimiters=<%s>\n", del?(char*)del:"(null)");
    printf("  noise=<%s>\n", noise?(char*)noise:"(null)");
    printf("  drop=<%s>\n", drop?(char*)drop:"(null)");
    printf("  conditional=<%s>\n", cond?(char*)cond:"(null)");
  }
}
int main(int c,char**v){ ace=LoadLibraryA(v[1]); ((U32(WINAPI*)(U16))F("AdsSetServerType"))(1);
  H conn=0; ((U32(WINAPI*)(U8*,U16,H*))F("AdsConnect26"))((U8*)v[2],1,&conn);
  const char*files[]={"docs.adt","opts.adt","fixed.adt"};
  typedef U32(WINAPI*OT)(H,U8*,U8*,U16,U16,U16,U16,U32,H*);
  for(int i=0;i<3;i++){ char p[300]; snprintf(p,300,"%s\\%s",v[2],files[i]);
    H t=0; U32 rc=((OT)F("AdsOpenTable"))(conn,(U8*)p,NULL,3,1,1,1,0,&t);
    if(rc){printf("%s open rc=%lu\n",files[i],(unsigned long)rc);continue;}
    info(t,files[i]); ((U32(WINAPI*)(H))F("AdsCloseTable"))(t);
  }
  return 0;}
