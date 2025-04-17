//stored in .card files

#include <fstream>
using std::iostream;
using std::ifsteam;

#include "paragraph.hpp"

class card{
  public:
    int get_length(){ return words; };
    void save_to_file();
    
    
  private:
    string identities[]; // U, L, !, LD, !D, FW, S
    long long unsigned int words;
    paragraph tag;
    paragraph cite;
    paragraph metadata;
    paragraph text;
}
