#include <string>

#include <iostream>
using std::cout;
using std::cerr;
using std::endl;

#include <deque>
using std::deuqe;

#include <fstream>
using std::ifstream;
using std::ofstream;

#include "card.h"
#include "paragraph.hpp"
#include "run.hpp"



int main(){
  // settings variables
  string vault_root; //path to root of application
  string stockpile_root; //path to directory that stores .card files
  unsigned int memory_to_use; //determines how much to load into memory 
  bool vim_mode; // bool to determine vim mode
  deque<string> open_file_names;

  return 0;
}
