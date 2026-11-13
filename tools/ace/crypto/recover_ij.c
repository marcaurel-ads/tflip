/* Batched probe that outputs recovered (i0,j0) directly (fast C brute).
   recover_ij.exe <dll> <datafolder> <records> <pwhex1> <pwhex2> ... -> JSON array
   Each password is given as HEX (so any byte sequence, incl. >20 bytes and
   non-ASCII, round-trips exactly between this probe and the Python analysis). For
   each pw: create ADT, encrypt, append N all-zero records, reopen w/o pw, read
   each record's keystream, recover (i0,j0) over KSA(pw[:20]) by brute.
   The KSA key is the raw password bytes truncated to 20 -- the SAME deliberate
   encoding and truncation the Python side uses (bytes.fromhex(pwhex)[:20]). */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
typedef unsigned long U32; typedef unsigned short U16; typedef unsigned char U8; typedef long S32; typedef ULONG_PTR H;
static HMODULE ace;
#define F(n) GetProcAddress(ace,n)
#define FIELD(n) ((U8*)(ULONG_PTR)(n))
#define KSA_MAX 20       /* ADS truncates the encryption password to 20 bytes */
static U8 zeros[250], rec[400];
typedef U32(WINAPI*OpenT)(H,U8*,U8*,U16,U16,U16,U16,U32,H*);
static void ksa(const U8*key,int kl,U8*s){
  for(int i=0;i<256;i++)s[i]=i;
  int j=0; for(int i=0;i<256;i++){ j=(j+s[i]+key[i%kl])&255; U8 t=s[i];s[i]=s[j];s[j]=t; }
}
/* does RC4(S0,i0,j0) produce ks[0..n-1]? */
static int match(const U8*S0,int i0,int j0,const U8*ks,int n){
  U8 s[256]; memcpy(s,S0,256); int i=i0,j=j0;
  for(int c=0;c<n;c++){ i=(i+1)&255; j=(j+s[i])&255; U8 t=s[i];s[i]=s[j];s[j]=t; if(s[(s[i]+s[j])&255]!=ks[c])return 0; }
  return 1;
}
/* hex -> bytes; returns byte length, or -1 on a bad/odd hex string. */
static int unhex(const char*h,U8*out,int max){
  int n=(int)strlen(h); if(n&1) return -1;
  int bl=n/2; if(bl>max) return -1;
  for(int i=0;i<bl;i++){ unsigned v; if(sscanf(h+2*i,"%2x",&v)!=1) return -1; out[i]=(U8)v; }
  return bl;
}
int main(int argc,char**argv){
  ace=LoadLibraryA(argv[1]); if(!ace)return 1;
  const char*data=argv[2]; int records=atoi(argv[3]);
  ((U32(WINAPI*)(U16))F("AdsSetServerType"))(1);
  printf("[");
  int emitted=0;
  for(int a=4;a<argc;a++){
    U8 pwb[256]; int pl=unhex(argv[a],pwb,255);
    if(pl<1) continue;               /* skip malformed or empty */
    pwb[pl]=0;                        /* NUL-terminate for the C-string API */
    char path[MAX_PATH]; snprintf(path,sizeof path,"%s\\ri%d.adt",data,a);
    H c=0; ((U32(WINAPI*)(U8*,U16,H*))F("AdsConnect26"))((U8*)data,1,&c);
    H t=0;
    ((U32(WINAPI*)(H,U8*,U8*,U16,U16,U16,U16,U16,U8*,U32,H*))F("AdsCreateTable71"))
      (c,(U8*)path,NULL,3,1,1,1,0,(U8*)"D1,Character,250",0,&t);
    ((U32(WINAPI*)(H,U8*))F("AdsEnableEncryption"))(t,pwb);
    ((U32(WINAPI*)(H))F("AdsEncryptTable"))(t);
    for(int r=0;r<records;r++){
      ((U32(WINAPI*)(H))F("AdsAppendRecord"))(t);
      ((U32(WINAPI*)(H,U8*,U8*,U32))F("AdsSetFieldRaw"))(t,FIELD(1),zeros,250);
      ((U32(WINAPI*)(H))F("AdsWriteRecord"))(t);
    }
    ((U32(WINAPI*)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI*)(H))F("AdsDisconnect"))(c);
    U8 S0[256]; ksa(pwb, pl<KSA_MAX?pl:KSA_MAX, S0);
    c=0; ((U32(WINAPI*)(U8*,U16,H*))F("AdsConnect26"))((U8*)data,1,&c);
    t=0; ((OpenT)F("AdsOpenTable"))(c,(U8*)path,NULL,3,1,1,1,0,&t);
    ((U32(WINAPI*)(H))F("AdsGotoTop"))(t);
    U32 cnt=0; ((U32(WINAPI*)(H,U16,U32*))F("AdsGetRecordCount"))(t,1,&cnt);
    printf("%s{\"pwhex\":\"%s\",\"ij\":[",emitted?",":"",argv[a]);
    emitted=1;
    for(U32 r=0;r<cnt;r++){
      U32 len=sizeof rec;
      if(((U32(WINAPI*)(H,U8*,U32*))F("AdsGetRecord"))(t,rec,&len))break;
      U8*ks=rec+5; int kn=(len-5)<12?(len-5):12;
      int fi=-1,fj=-1;
      for(int i0=0;i0<256&&fi<0;i0++)for(int j0=0;j0<256;j0++) if(match(S0,i0,j0,ks,kn)){fi=i0;fj=j0;break;}
      printf("%s[%d,%d]",r?",":"",fi,fj);
      if(((U32(WINAPI*)(H,S32))F("AdsSkip"))(t,1))break;
    }
    printf("]}");
    ((U32(WINAPI*)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI*)(H))F("AdsDisconnect"))(c);
    DeleteFileA(path);
  }
  printf("]\n"); return 0;
}
