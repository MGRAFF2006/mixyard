#include <cassert>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <string>
#include "mixyard_host.h"
static bool live = false;
static unsigned operations = 0;
extern "C" void* __real_malloc(size_t);
extern "C" void* __real_calloc(size_t,size_t);
extern "C" void* __real_realloc(void*,size_t);
extern "C" void __real_free(void*);
extern "C" void* __wrap_malloc(size_t n) { if(live) ++operations; return __real_malloc(n); }
extern "C" void* __wrap_calloc(size_t n,size_t s) { if(live) ++operations; return __real_calloc(n,s); }
extern "C" void* __wrap_realloc(void* p,size_t n) { if(live) ++operations; return __real_realloc(p,n); }
extern "C" void __wrap_free(void* p) { if(live) ++operations; __real_free(p); }
int main(int argc,char**argv) {
 assert(argc==2);std::ifstream f(argv[1]);std::string json((std::istreambuf_iterator<char>(f)),{});
 mx_host* h=nullptr;char error[2048]{};int result=mx_host_create(reinterpret_cast<const uint8_t*>(json.data()),json.size(),&h,error,sizeof error);
 if(result) { std::fprintf(stderr,"create: %d %s\n",result,error); return 1; }
 assert(mx_host_source_count(h)==3 && mx_host_output_count(h)==2);
 double pcm[3][4]{{0.9,0.9,0.9,0.9},{0.2,-0.2,0.4,-0.4},{0.4,-0.4,0.8,-0.8}};
 for(uint64_t seq=0;seq<512;++seq) {
   uint64_t now=100+seq*50000;mx_stamp stamp{7,1,11,seq,seq*2,now,now+1000000};mx_input inputs[3]{};
   for(unsigned i=0;i<3;++i) inputs[i]={reinterpret_cast<const uint8_t*>(pcm[i]),sizeof pcm[i],MX_F64LE,48000,2,stamp};
   live=true;assert(mx_host_process(h,now,stamp,2,inputs,3)==MX_OK);
   const double* samples=nullptr;size_t count=0;assert(mx_host_output(h,0,&samples,&count)==MX_OK &&count==4);
   assert(samples[0]==0.4 && samples[1]==-0.4 &&samples[2]==0.8 &&samples[3]==-0.8);
   assert(mx_host_output(h,1,&samples,&count)==MX_OK &&count==4);
   assert(samples[0]==0.125 &&samples[1]==-0.125 &&samples[2]==0.25 &&samples[3]==-0.25);live=false;
 }
 mx_stamp stale{7,2,11,512,1024,30000000,31000000};mx_input input[3]{};
 live=true;assert(mx_host_process(h,30000000,stale,2,input,3)==MX_STALE);
 const double* samples=reinterpret_cast<double*>(1);size_t count=99;
 assert(mx_host_output(h,0,&samples,&count)==MX_INACTIVE && samples==nullptr &&count==0);
 mx_host_revoke(h);live=false;assert(operations==0);mx_host_destroy(h);
 std::puts("PASS actual static Rust C ABI: 512 mixed/DSP blocks, private/mix-minus, stale/revoke and zero malloc/calloc/realloc/free calls");
}
