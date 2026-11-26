// SPDX-License-Identifier: GPL-3.0-or-later
// Isolated C-ABI probe: no server, input-method registration or user data.
#include <windows.h>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <string>
#include <vector>
using Create = int (__cdecl*)(const char*,void**);
using Score = int (__cdecl*)(void*,const char* const*,int,double*);
using Destroy = void (__cdecl*)(void*);
static std::string utf8(const wchar_t* text) {
    int n = WideCharToMultiByte(CP_UTF8,0,text,-1,nullptr,0,nullptr,nullptr);
    std::string out(n, '\0');
    WideCharToMultiByte(CP_UTF8,0,text,-1,out.data(),n,nullptr,nullptr);
    out.pop_back(); return out;
}
int wmain(int argc,wchar_t** argv) {
    if(argc!=5) { std::cerr<<"score-probe <dll> <model> <UTF8-lines> <out.tsv>\n"; return 1; }
    HMODULE dll=LoadLibraryW(argv[1]);
    if(!dll) { std::cerr<<"LoadLibrary error="<<GetLastError()<<"\n"; return 2; }
    auto create=reinterpret_cast<Create>(GetProcAddress(dll,"tcs_create_from_file"));
    auto score=reinterpret_cast<Score>(GetProcAddress(dll,"tcs_score"));
    auto destroy=reinterpret_cast<Destroy>(GetProcAddress(dll,"tcs_destroy"));
    if(!create||!score||!destroy) { FreeLibrary(dll); return 3; }
    std::ifstream in{std::filesystem::path(argv[3])};
    std::vector<std::string> candidates;std::string line;
    while(std::getline(in,line)) { if(!line.empty()&&line.back()=='\r')line.pop_back(); if(!line.empty())candidates.push_back(line); }
    if(candidates.empty()) { FreeLibrary(dll); return 4; }
    void* scorer=nullptr;int rc=create(utf8(argv[2]).c_str(),&scorer);
    if(rc||!scorer) { std::cerr<<"create rc="<<rc<<"\n"; FreeLibrary(dll); return 5; }
    std::ofstream out{std::filesystem::path(argv[4])};out<<std::setprecision(17);
    for(size_t base=0;base<candidates.size();base+=5) {
        size_t count=std::min<size_t>(5,candidates.size()-base);
        std::vector<const char*> pointers;for(size_t i=0;i<count;i++)pointers.push_back(candidates[base+i].c_str());
        std::vector<double> values(count);rc=score(scorer,pointers.data(),static_cast<int>(count),values.data());
        if(rc) {std::cerr<<"score rc="<<rc<<" batch="<<base/5<<"\n";destroy(scorer);FreeLibrary(dll);return 6;}
        for(size_t i=0;i<count;i++)out<<base+i<<'\t'<<values[i]<<'\n';
    }
    out.flush();bool good=out.good();destroy(scorer);FreeLibrary(dll);return good?0:7;
}
