#include <string>
using std::to_string();

class run{
  public:
    run(){ 
      text="";
      highlight=false;
      highlight_color="yellow";
      underline=false;
      bold=false;
      size=0;
    }

    run(const bool &hl, const string &hlc, const bool &u, const bool &b, const unsigned char &sz, const bool &w){
      text="";
      highlight=hl;
      highlight_color=hlc;
      underline=u;
      bold=b;
      size=sz;
      whitespace_preseve=w;
    }

    void add_word(const string &word){
      text = text + " " + word;
    }
    void set_highlight(const bool &hl){ highlight=hl; }
    void set_highlight_color(const string &hlc){ highlight_color=hlc; }
    void set_underline(const bool &u){ underline=u; }
    void set_bold(const bool &b){ bold=b; }
    void set_size(const unsigned char &sz){ size=sz; }

    bool get_highlight(){ return highlight; }
    string get_highlight_color(){return highlight_color; }
    bool get_underline(){ return underline; }
    unsigned char get_size(){ return size; }
    string get_text(){ return text; }

    string get_formatting(){
      return to_string((int)highlight) + to_string((int)underline) + to_string((int)bold) + to_string((int)size);
    }

    string to_xml(){
          //if it's a cite format
      if(size==13 && bold){ 
        return "<w:r><w:rPr><w:rStyle w:val=\"Style13ptBold\"/></w:rPr><w:t>" + text + "</w:t></w:r>";
      }

            //non-cite formatting
      string xml = "<w:r><w:rPr><w:rStyle w:val=\"StyleUnderline\"/>";
      if(bold){
        xml += "<w:b/>";
      }
      if(highlight){
        xml += ("<w:highlight w:val=\"" + highlight_color + "\"/>");
      }
      if(size!=24){
        xml += "<w:sz w:val=\"" + size + "\"/>" + "<w:szCs w:val=\"" + size + "\"/>";
      }
      if(!underline){
        xml += "<w:u w:val=\"none\"/>";
      }
      xml += "<w:t>" + text + "</w:t></w:r>";
      return xml;
    }

  private:
    string text;
    bool highlight;
    string highlight_color;
    bool underline;
    bool bold;
    unsigned char size;
    bool whitespace_preseve;
}
