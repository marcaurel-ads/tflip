/* Probe AdsGetKeyFilter: AutoInc fallback, ODATA suffixes & date-T, EOF, options. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
typedef unsigned long U32; typedef unsigned short U16; typedef unsigned char U8; typedef long S32; typedef ULONG_PTR H;
static HMODULE ace;
#define F(n) GetProcAddress(ace, n)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define ADS_UNIQUE 1
#define ADS_ADT 3
static void err(const char*w,U32 rc){U8 t[400];U16 l=sizeof t;U32 c=0;
  ((U32(WINAPI*)(U32*,U8*,U16*))F("AdsGetLastError"))(&c,t,&l);printf("%s rc=%lu(%lu)\n",w,(unsigned long)rc,(unsigned long)c);}
static U32 setf(H t,int f,const char*v){return ((U32(WINAPI*)(H,U8*,U8*,U32))F("AdsSetField"))(t,FIELD(f),(U8*)v,strlen(v));}
static void kf(H t,const char*label,U32 opt){U8 b[512];U32 l=sizeof b;
  U32 rc=((U32(WINAPI*)(H,U8*,U32,U8*,U32*))F("AdsGetKeyFilter"))(t,NULL,opt,b,&l);
  if(rc){printf("  %-18s ",label);err("",rc);return;} b[l]=0;printf("  %-18s <<%s>>\n",label,b);}
static H ci; /* index creator */
int main(int argc,char**argv){
  ace=LoadLibraryA(argv[1]); ((U32(WINAPI*)(U16))F("AdsSetServerType"))(1);
  const char*folder=argv[2]; H conn=0;
  ((U32(WINAPI*)(U8*,U16,H*))F("AdsConnect26"))((U8*)folder,1,&conn);
  typedef U32(WINAPI*CI)(H,U8*,U8*,U8*,U8*,U8*,U32,H*); CI mk=(CI)F("AdsCreateIndex");
  char p[260],a[260]; H h=0;

  /* AutoInc fallback: a table with an AutoInc field and no unique index. */
  snprintf(p,260,"%s\\ai.adt",folder); snprintf(a,260,"%s\\ai.adi",folder);
  H t=0; ((U32(WINAPI*)(H,U8*,U8*,U16,U16,U16,U16,U16,U8*,U32,H*))F("AdsCreateTable71"))
    (conn,(U8*)p,NULL,ADS_ADT,1,1,1,0,(U8*)"n,Character,5;ac,Autoinc",0,&t);
  mk(t,(U8*)a,(U8*)"BYN",(U8*)"n",NULL,NULL,0,&h); /* non-unique only */
  ((U32(WINAPI*)(H))F("AdsAppendRecord"))(t); setf(t,1,"aa"); ((U32(WINAPI*)(H))F("AdsWriteRecord"))(t);
  ((U32(WINAPI*)(H))F("AdsAppendRecord"))(t); setf(t,1,"bb"); ((U32(WINAPI*)(H))F("AdsWriteRecord"))(t);
  ((U32(WINAPI*)(H))F("AdsGotoTop"))(t); ((U32(WINAPI*)(H,S32))F("AdsSkip"))(t,1);
  printf("autoinc table (no unique), record 2:\n");
  kf(t,"default",0); kf(t,"odata",1);
  ((U32(WINAPI*)(H,S32))F("AdsSkip"))(t,5); /* to EOF */
  kf(t,"at EOF",0);

  /* Money + Numeric unique, for ODATA suffixes. */
  snprintf(p,260,"%s\\mn.adt",folder); snprintf(a,260,"%s\\mn.adi",folder);
  H t2=0; ((U32(WINAPI*)(H,U8*,U8*,U16,U16,U16,U16,U16,U8*,U32,H*))F("AdsCreateTable71"))
    (conn,(U8*)p,NULL,ADS_ADT,1,1,1,0,(U8*)"m,Money;q,Numeric,10,2",0,&t2);
  mk(t2,(U8*)a,(U8*)"UM",(U8*)"m",NULL,NULL,ADS_UNIQUE,&h);
  mk(t2,(U8*)a,(U8*)"VQ",(U8*)"q",NULL,NULL,ADS_UNIQUE,&h);
  ((U32(WINAPI*)(H))F("AdsAppendRecord"))(t2); setf(t2,1,"12.50"); setf(t2,2,"3.14"); ((U32(WINAPI*)(H))F("AdsWriteRecord"))(t2);
  ((U32(WINAPI*)(H))F("AdsGotoTop"))(t2);
  printf("money/numeric unique (UM sorts first):\n");
  kf(t2,"default",0); kf(t2,"odata",1);

  /* Date ODATA T-separator. */
  snprintf(p,260,"%s\\dt.adt",folder); snprintf(a,260,"%s\\dt.adi",folder);
  ((U32(WINAPI*)(U8*))F("AdsSetDateFormat"))((U8*)"CCYYMMDD");
  H t3=0; ((U32(WINAPI*)(H,U8*,U8*,U16,U16,U16,U16,U16,U8*,U32,H*))F("AdsCreateTable71"))
    (conn,(U8*)p,NULL,ADS_ADT,1,1,1,0,(U8*)"d,Date",0,&t3);
  mk(t3,(U8*)a,(U8*)"UD",(U8*)"d",NULL,NULL,ADS_UNIQUE,&h);
  ((U32(WINAPI*)(H))F("AdsAppendRecord"))(t3); setf(t3,1,"20200102"); ((U32(WINAPI*)(H))F("AdsWriteRecord"))(t3);
  ((U32(WINAPI*)(H))F("AdsGotoTop"))(t3);
  printf("date unique:\n");
  kf(t3,"default",0); kf(t3,"odata",1);

  /* Options above 3. */
  printf("options:\n");
  kf(t3,"opt=2 (utf8)",2); kf(t3,"opt=4",4); kf(t3,"opt=8",8);
  return 0;
}
