#include <windows.h>
#include <msctf.h>
#include <psapi.h>
#include <cstdio>
static const GUID cid={0x8f5c2a10,0x3e77,0x4b9c,{0xa1,0xd4,0x9e,0x0b,0x7c,0x2f,0x5a,0x88}};
static const GUID pid={0x8f5c2a11,0x3e77,0x4b9c,{0xa1,0xd4,0x9e,0x0b,0x7c,0x2f,0x5a,0x88}};
int wmain(){
 CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
 ITfInputProcessorProfileMgr* mgr{};auto hr=CoCreateInstance(CLSID_TF_InputProcessorProfiles,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&mgr));if(FAILED(hr))return 1;
 TF_INPUTPROCESSORPROFILE profile{};hr=mgr->GetProfile(TF_PROFILETYPE_INPUTPROCESSOR,0x804,cid,pid,nullptr,&profile);printf("GetProfile=%08X flags=%08X\n",unsigned(hr),profile.dwFlags);if(FAILED(hr))return 2;
 ITfInputProcessorProfiles* p{};mgr->QueryInterface(IID_PPV_ARGS(&p));BOOL enabled=FALSE;hr=p->IsEnabledLanguageProfile(cid,0x804,pid,&enabled);printf("IsEnabled=%08X enabled=%d\n",unsigned(hr),enabled);p->Release();mgr->Release();if(FAILED(hr)||!enabled)return 3;
 ITfTextInputProcessorEx* tip{};hr=CoCreateInstance(cid,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&tip));printf("CoCreateInstance(registered HuFu ITfTextInputProcessorEx)=%08X\n",unsigned(hr));if(FAILED(hr))return 4;
 HMODULE modules[1024]{};DWORD bytes=0;if(EnumProcessModules(GetCurrentProcess(),modules,sizeof(modules),&bytes))for(unsigned i=0;i<bytes/sizeof(HMODULE);++i){wchar_t path[32768]{};GetModuleFileNameW(modules[i],path,32768);if(wcsstr(path,L"hufu")||wcsstr(path,L"HuFu"))wprintf(L"loaded=%s\n",path);}
 tip->Release();CoUninitialize();return 0;
}
