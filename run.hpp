#include <string>
using std::to_string();

class run{
  public:
    run(){ 
      text="" ;
      highlight=false;
      underline=false;
      heading_level=0;
      size=0;
    }

    run(const bool $hl, const bool $u, const unsigned char $lv, const unsigned char &sz, const bool &w){
      text="";
      highlight=hl;
      underline=u;
      heading_level=lv;
      size=sz;
      whitespace_preseve=w;
    }

    void add_word(const string &word){
      text = text + " " + word;
    }

    void set_highlight(const bool &hl){
      highlight=hl;
    }

    void set_underline(const bool &u){
      underline=u;
    }

    void set_heading(const unsigned char &lv){
      heading_level=lv;
    }

    void set_size(const unsigned char &sz){
      size=sz;
    }

    string get_text(){
      return text;
    }

    string get_formatting(){
      return to_string((int)highlight) + to_string((int)underline) + to_string((int)heading_level) + to_string((int)size)
    }

    string to_xml(){
    return ""
    }

    

  private:
    string text;
    bool highlight;
    bool underline;
    unsigned char heading_level;
    unsigned char size;
    bool whitespace_preseve;
}
