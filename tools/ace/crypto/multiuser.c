#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>
typedef unsigned long U32; typedef unsigned short U16; typedef unsigned char U8; typedef ULONG_PTR H;
static HMODULE ace; 
#define F(n) GetProcAddress(ace,n)
static long rf(const char*p,U8*b,long m){FILE*f=fopen(p,"rb");if(!f)return -1;long n=fread(b,1,m,f);fclose(f);return n;}
int main(int argc,char**argv){
  /* multiuser.exe <dll> <datafolder> <tag> <userpw1> <userpw2> ... */
  ace=LoadLibraryA(argv[1]); if(!ace)return 1;
  ((U32(WINAPI*)(U16))F("AdsSetServerType"))(1);
  char path[MAX_PATH]; snprintf(path,sizeof path,"%s\\mu%s.add",argv[2],argv[3]);
  H d=0; ((U32(WINAPI*)(U8*,U16,U8*,H*))F("AdsDDCreate"))((U8*)path,0,(U8*)"p",&d);
  for(int k=4;k<argc;k++){
    char uname[16]; snprintf(uname,sizeof uname,"U%d",k-3);
    ((U32(WINAPI*)(H,U8*,U8*,U8*,U8*))F("AdsDDCreateUser"))(d,NULL,(U8*)uname,(U8*)argv[k],(U8*)"u");
  }
  ((U32(WINAPI*)(H))F("AdsDisconnect"))(d);
  static U8 buf[1<<18]; long fl=rf(path,buf,sizeof buf);
  printf("{\"len\":%ld,\"hex\":\"",fl);
  for(long i=0;i<fl;i++)printf("%02x",buf[i]);
  printf("\"}\n"); return 0;
}
